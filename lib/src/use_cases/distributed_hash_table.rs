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
use crate::use_cases::{EventHandler, ReactiveUseCaseState, UseCase, UseCaseEvent};

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
        Instant::now().duration_since(time) >= self.expire_after
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

pub struct DistributedHashTable<C, D, EC, S, H>
{
    _c: PhantomData<C>,
    _d: PhantomData<D>,
    state: ReactiveUseCaseState,
    config: DistributedHashTableConfig<S, H>,
}

impl<C, D, EC, S, H> DistributedHashTable<C, D, EC, S, H>
{
    pub fn new(config: DistributedHashTableConfig<S, H>) -> Self {
        Self {
            _c: Default::default(),
            _d: Default::default(),
            state: Default::default(),
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
        Self {
            _c: Default::default(),
            _d: Default::default(),
            state: Default::default(),
            config: Default::default(),
        }
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
        match event {
            // StoreReq
            UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _) => {
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

                Ok(())
            }
            UseCaseEvent::Message(ProtocolMessage::FetchRep(req), _) => {
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

                Ok(())
            }
            // todo handle timer for periodic hashtable collection
            _ => Ok(())
        }
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
    type State = ReactiveUseCaseState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        self.state = Self::State::default();
        Ok(())
        // todo start timer for periodic hashtable collection
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    // todo implement tests
}
