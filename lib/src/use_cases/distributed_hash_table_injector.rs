use std::fmt::Debug;
use core::time::Duration;
use std::collections::{HashMap, LinkedList};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::time::Instant;

use crate::context::UseCaseContext;
use crate::domain::{GroupingError, node_id, NodeId, RoutingTable};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, InjectionMessageData, OneshotInjectMessageCallback, TimerId, UseCase, UseCaseEvent, UseCaseState};

use crate::messaging::{Nonce, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::messaging::dht::{DefaultLHTInput, FetchReqData, StoreReqData};
use crate::use_cases::distributed_hash_table_injector::DHTInjectorState::Running;
use crate::use_cases::inject_messages::InjectionResult;
use crate::use_cases::inject_messages::errors::InjectMessageError;

// todo what would be a sensible value here?
// todo random offsets for periodic restore AND collection of the hash table?
pub const DEFAULT_PERIODIC_RESTORE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableInjectorConfig
{
    periodic_restore: Duration,
    shared_prefix_grouping: NonZeroUsize,
}

impl Default for DistributedHashTableInjectorConfig {
    fn default() -> Self {
        Self {
            periodic_restore: DEFAULT_PERIODIC_RESTORE,
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTInjectorState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

pub struct DistributedHashTableInjector<C, const BUCKET_SIZE: usize>
{
    _c: PhantomData<C>,
    state: DHTInjectorState,
    config: DistributedHashTableInjectorConfig,
    nonces: HashMap<Nonce, (Instant, OneshotInjectMessageCallback)>,
    restore_data: LinkedList<StoreReqData<DefaultLHTInput>>,
}

impl UseCaseState for DHTInjectorState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE>
{
    pub fn new(config: DistributedHashTableInjectorConfig) -> Result<Self, GroupingError> {
        if config.shared_prefix_grouping.get() > node_id::BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_grouping.get(),
                id_size: node_id::BIT_SIZE,
            });
        }

        Ok(Self {
            _c: PhantomData,
            state: DHTInjectorState::default(),
            config,
            nonces: HashMap::default(),
            restore_data: LinkedList::default(),
        })
    }
}

impl<C, const BUCKET_SIZE: usize> Default for DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(DistributedHashTableInjectorConfig::default())
            .expect("default grouping has to be valid")
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE>
    where
        C: UseCaseContext,
        for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
        C::MessageSender: ProtocolMessageSender
{
    fn construct_req_rsp_msg<T: Debug>(&self, context: &C, nonce: Nonce, data: T, destination: NodeId) -> ReqRspMessage<T> {
        let source_route = context
            .routing_table()
            .next_source_route(&destination, 20, self.config.shared_prefix_grouping.get());

        ReqRspMessage {
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            not_via: context.not_via().clone(),
            data,
            nonce,
            source_route,
        }
    }

    fn send_store_req(&self, context: &C, nonce: Nonce, data: StoreReqData<DefaultLHTInput>) -> Result<(), InjectMessageError> {
        let dest = data.handle.clone();
        let message = self.construct_req_rsp_msg(context, nonce, data, dest);

        log::trace!(target: "inject_messages", "Sending StoreReq from {} with target {}",
            message.source_route.source(),
            message.source_route.destination()
        );

        let message = ProtocolMessage::StoreReq(message);
        context.message_sender_mut().send_message(message).map_err(|e| {
            log::error!(target: "inject_messages", "Failed to send triggered message: {}", e);
            InjectMessageError::SendFailed
        })
    }

    fn send_fetch_req(&self, context: &C, nonce: Nonce, data: FetchReqData) -> Result<(), InjectMessageError> {
        let dest = data.handle.clone();
        let message = self.construct_req_rsp_msg(context, nonce, data, dest);

        log::trace!(target: "inject_messages", "Sending FetchReq from {} with target {}",
            message.source_route.source(),
            message.source_route.destination()
        );

        let message = ProtocolMessage::FetchReq(message);
        context.message_sender_mut().send_message(message).map_err(|e| {
            log::error!(target: "inject_messages", "Failed to send triggered message: {}", e);
            InjectMessageError::SendFailed
        })
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn send_inject_result(&self, result: InjectionResult, callback: OneshotInjectMessageCallback) -> Result<(), InjectMessageError> {
        callback.send(result).map_err(|e| {
            log::error!(target: "inject_messages", "failed to send inject result: {:#?}", e);

            InjectMessageError::SendResultFailed
        })
    }

    fn inform_injector_about_send_error(&self, injection_error: InjectMessageError, nonce: &Nonce, callback: OneshotInjectMessageCallback) -> Result<(), InjectMessageError> {
        match injection_error {
            InjectMessageError::Isolated => {
                self.send_inject_result(InjectionResult::Isolated, callback)?
            }
            InjectMessageError::SendFailed => {
                self.send_inject_result(InjectionResult::SendFailed(nonce.clone()), callback)?
            }
            InjectMessageError::SendResultFailed => {
                // no information since the cause was that we couldn't reach the injector
                return Err(InjectMessageError::SendResultFailed);
            }
        }

        Ok(())
    }
}


impl<C, const BUCKET_SIZE: usize> EventHandler for DistributedHashTableInjector<C, BUCKET_SIZE>
    where
        C: UseCaseContext,
        for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
{
    type Context = C;
    type Error = InjectMessageError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::InjectMessage(nonce, InjectionMessageData::Store(payload, callback)), _) => {
                if let Err(inject_err) = self.send_store_req(context, nonce.clone(), payload.data.clone()) {
                    self.inform_injector_about_send_error(inject_err, &nonce, callback)?;
                } else {
                    self.nonces.insert(nonce.clone(), (Instant::now(), callback));
                    if payload.restore {
                        self.restore_data.push_back(payload.data);
                    }
                }
            }
            (UseCaseEvent::InjectMessage(nonce, InjectionMessageData::Fetch(payload, callback)), _) => {
                if let Err(inject_err) = self.send_fetch_req(context, nonce.clone(), payload) {
                    self.inform_injector_about_send_error(inject_err, &nonce, callback)?;
                } else {
                    self.nonces.insert(nonce.clone(), (Instant::now(), callback.clone()));
                }
            }
            (UseCaseEvent::Message(message, interface), _) => {
                // don't react on looped requests
                match message {
                    ProtocolMessage::StoreReq(_) | ProtocolMessage::FetchReq(_) => return Ok(()),
                    _ => {}
                }

                if let Some(Some((instant, callback))) = message.nonce().map(|nonce| self.nonces.remove(nonce))
                {
                    let elapsed = instant.elapsed();
                    self.send_inject_result(InjectionResult::Answered((message.clone(), interface)), callback)?;

                    log::trace!(target: "inject_messages", "Received response for nonce {:?} after {:?}", message.nonce(), elapsed);
                }
            }
            (UseCaseEvent::Timer(id), Running(our_id)) => {
                if &id == our_id {
                    for data in self.restore_data.iter() {
                        // todo make this more efficient
                        // 1.) calculate source routes only once for every handle
                        // 2.) pack all data to that node into a single request
                        self.send_store_req(context, Nonce::random(), data.clone())?;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, const BUCKET_SIZE: usize> UseCase for DistributedHashTableInjector<C, BUCKET_SIZE>
    where
        C: UseCaseContext,
        for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
{
    type State = DHTInjectorState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let timer_id = context.runtime().register_periodic_timer(self.config.periodic_restore);

        self.state = Running(timer_id);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    // todo implement tests
}
