use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::time::Instant;
use core::time::Duration;
use std::marker::PhantomData;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use crate::context::UseCaseContext;
use crate::domain::dht::{Expiring, HashTable, TimeoutStrategy};
use crate::domain::{NodeId, StateSeqNr};
use crate::domain::dht::expiring_hash_table::ExpiringHashTable;
use crate::messaging::{ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::messaging::dht_messaging::{FetchErr, FetchRspData, StoreErr, StoreRspData};
use crate::messaging::error::SenderError;
use crate::messaging::source_route::SourceRoute;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24);
pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig<S, H>
{
    strategy: S,
    hash_table: H,
    collect_interval: Duration,
}

impl<H, EC, D> Default for DistributedHashTableConfig<ConstTimeoutStrategy, H>
    where
        H: HashTable<NodeId, D> + Expiring<EC>,
{
    fn default() -> Self {
        Self {
            strategy: ConstTimeoutStrategy::default(),
            hash_table: ExpiringHashTable::default(),
            collect_interval: DEFAULT_COLLECT_INTERVAL,
        }
    }
}


struct ConstTimeoutStrategy {
    expire_after: Duration,
}

impl Default for ConstTimeoutStrategy {
    fn default() -> Self {
        Self {
            expire_after: DEFAULT_TIMEOUT,
        }
    }
}

impl<C> TimeoutStrategy<C> for ConstTimeoutStrategy {
    fn is_timed_out(&self, context: C, time: &Instant) -> bool {
        if let Some(duration) = Instant::now().checked_duration_since(*time) {
            duration >= self.expire_after
        } else {
            false
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum DHTError {
    DHTSendError(SenderError)
}

impl Display for DHTError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl Error for DHTError {}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for DHTState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

pub struct DistributedHashTable<C, D, EC, S, H>
{
    _c: PhantomData<C>,
    _d: PhantomData<D>,
    state: DHTState,
    config: DistributedHashTableConfig<S, H>,
}

impl<C, D, EC, S, H> DistributedHashTable<C, D, EC, S, H>
{
    pub fn new(config: DistributedHashTableConfig<S, H>) -> Self {
        Self {
            _c: PhantomData::default(),
            _d: PhantomData::default(),
            state: DHTState::default(),
            config,
        }
    }
}

impl<C, D, EC, H> Default for DistributedHashTable<C, D, EC, ConstTimeoutStrategy, H>
    where
            for<'a> D: Serialize + Deserialize<'a>,
            H: HashTable<NodeId, D> + Expiring<EC>,
{
    fn default() -> Self {
        Self::new(DistributedHashTableConfig::default())
    }
}

impl<C, D, EC, S, H> EventHandler for DistributedHashTable<C, D, EC, S, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        for<'a> D: Serialize + Deserialize<'a> + Debug,
        H: HashTable<NodeId, D, StoreErr=StoreErr, FetchErr=FetchErr> + Expiring<EC>,
        S: TimeoutStrategy<EC>
{
    type Context = C;
    type Error = DHTError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                let res = self.config.hash_table.store(context.root_id().clone(), req.data.data);
                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: StateSeqNr::from(0), // <-- todo what here?
                    data: StoreRspData {
                        status: res
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from_reversed(req.source_route), // todo is this right?
                    // maybe look into similar req -> send rsp occurrences
                };

                log::trace!(
                    target: "dht",
                    "Sending message: {:?}",
                    rsp
                );

                if let Err(e) = context.message_sender_mut().send_message(rsp) {
                    log::error!("Failed to send message: {}", e);
                    return Err(DHTError::DHTSendError(e)); // todo return more descriptive error
                }
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchRep(req), _), _) => {
                let fetch_res = self.config.hash_table.fetch(context.root_id());
                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: StateSeqNr::from(0),
                    data: FetchRspData {
                        data: fetch_res,
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from_reversed(req.source_route),
                };

                log::trace!(
                    target: "dht",
                    "Sending message: {:?}",
                    rsp
                );

                if let Err(e) = context.message_sender_mut().send_message(rsp) {
                    log::error!("Failed to send message: {}", e);
                    return Err(DHTError::DHTSendError(e));
                }
            }
            (UseCaseEvent::Timer(id), DHTState::Running(our_timer_id)) => {
                if &id == our_timer_id {
                    self.config.hash_table.expire_with_strategy(&self.config.strategy);
                }
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, D, EC, S, H> UseCase for DistributedHashTable<C, D, EC, S, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        for<'a> D: Serialize + Deserialize<'a> + Debug,
        H: HashTable<NodeId, D, StoreErr=StoreErr, FetchErr=FetchErr> + Expiring<EC>,
        S: TimeoutStrategy<EC>
{
    type State = DHTState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_timer(self.config.collect_interval);

        self.state = DHTState::Running(timer_id);

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
