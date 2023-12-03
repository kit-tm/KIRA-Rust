use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use core::time::Duration;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::time::Instant;

use crate::context::UseCaseContext;
use crate::domain::RoutingTable;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, InjectionMessageData, TimerId, UseCase, UseCaseEvent, UseCaseState};

use crate::messaging::{Nonce, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::messaging::source_route::SourceRoute;
use crate::use_cases::inject_messages::InjectionResultSender;

// todo what would be a sensible value here?
// todo random offsets for periodic restore AND collection of the hash table?
pub const DEFAULT_PERIODIC_RESTORE: Duration = Duration::from_secs(60 * 60);
pub const DEFAULT_SEND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableInjectorConfig
{
    periodic_restore: Duration,
    send_timeout: Duration,
}

impl Default for DistributedHashTableInjectorConfig {
    fn default() -> Self {
        Self {
            periodic_restore: DEFAULT_PERIODIC_RESTORE,
            send_timeout: DEFAULT_SEND_TIMEOUT,
        }
    }
}

#[derive(Debug)]
pub enum DHTInjectError {
    SendResultFailed,
    SendFailed,
    Isolated,
}

impl Display for DHTInjectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl Error for DHTInjectError {}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTInjectorState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

pub struct DistributedHashTableInjector<C, IRS, const BUCKET_SIZE: usize>
{
    _c: PhantomData<C>,
    state: DHTInjectorState,
    config: DistributedHashTableInjectorConfig,
    injection_result_sender: IRS,
    nonces: HashMap<Nonce, Instant>,
}

impl UseCaseState for DHTInjectorState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, IRS, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, IRS, BUCKET_SIZE>
{
    pub fn new(config: DistributedHashTableInjectorConfig, sender: IRS) -> Self {
        Self {
            _c: PhantomData::default(),
            state: DHTInjectorState::default(),
            config,
            injection_result_sender: sender,
            nonces: HashMap::default(),
        }
    }

    pub fn with_default_config(sender: IRS) -> Self {
        Self::new(DistributedHashTableInjectorConfig::default(), sender)
    }
}

impl<C, IRS, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, IRS, BUCKET_SIZE>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime
{
    fn restore(&mut self) {}

    fn start_restore_timer(&mut self, context: &C) {
        let timer_id = context
            .runtime()
            .register_timer(self.config.periodic_restore);

        self.state = DHTInjectorState::Running(timer_id);
    }
}

impl<C, IRS, const BUCKET_SIZE: usize> EventHandler for DistributedHashTableInjector<C, IRS, BUCKET_SIZE>
    where
        C: UseCaseContext,
        for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        IRS: InjectionResultSender,
{
    type Context = C;
    type Error = DHTInjectError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::InjectMessage(nonce, InjectionMessageData::Store(data)), _) => {
                let closest_route = context
                    .routing_table()
                    .closest(&data.handle, 20, self.config.shared_prefix_grouping.get())
                    .expect("grouping has to be checked on init")
                    .first()
                    .map(|(_, contact)| {
                        let mut route = SourceRoute::from(contact.path().clone());
                        route.push_front(context.root_id().clone());
                        route
                    });

                let message = ProtocolMessage::StoreReq(ReqRspMessage {
                    nonce: nonce.clone(),
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data: data,
                    not_via: context.not_via().clone(),
                    source_route:,
                });

                context.message_sender_mut().send_message(message);

                self.nonces.insert(nonce.clone(), Instant::now());
            }
            (UseCaseEvent::Timer(id), DHTInjectorState::Running(our_id)) => {
                if &id == our_id {
                    self.restore();
                    self.start_restore_timer(context);
                }
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, IRS> UseCase for DistributedHashTableInjector<C, IRS>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        IRS: InjectionResultSender,
{
    type State = DHTInjectorState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        self.start_restore_timer(context);

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
