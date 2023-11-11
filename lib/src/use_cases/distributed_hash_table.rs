use std::error::Error;
use std::fmt::{Display, Formatter};
use std::time::Instant;
use core::time::Duration;
use std::marker::PhantomData;
use serde::{Serialize};
use serde::de::DeserializeOwned;
use crate::context::UseCaseContext;
use crate::domain::dht::{Expiring, HashTable, TimeoutStrategy};
use crate::domain::{NodeId, StateSeqNr};
use crate::messaging::{ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::messaging::dht_messaging::{FetchErr, FetchReqData, FetchRspData, StoreErr, StoreReqData, StoreRspData};
use crate::messaging::source_route::SourceRoute;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, ReactiveUseCaseState, UseCase, UseCaseEvent, UseCaseState};
use crate::use_cases::distributed_hash_table::DHTError::DHTError;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24);
pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig<C, D, S, H>
    where
        H: HashTable<NodeId, D> + Expiring<C>,
        S: TimeoutStrategy<C>
{
    strategy: S,
    hash_table: H,
    collect_interval: Duration,
}

impl<D, H> Default for DistributedHashTableConfig<&NodeId, D, ConstTimeoutStrategy, H>
    where
        H: HashTable<NodeId, D> + Expiring<&NodeId>,
{
    fn default() -> Self {
        Self {
            strategy: Default::default(),
            hash_table: (), // todo implement default hash table
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
    fn is_timed_out(&self, context: C, time: Instant) -> bool {
        Instant::now().duration_since(time) >= self.expire_after
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum DHTError {
    DHTError(Err)
}

impl Display for DHTError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl Error for DHTError {}

pub struct DistributedHashTable<C, D, EC, S, H>
    where
        D: Serialize + DeserializeOwned,
        H: HashTable<&NodeId, D> + Expiring<EC>,
        S: TimeoutStrategy<EC>
{
    _c: PhantomData<C>,
    state: ReactiveUseCaseState,
    config: DistributedHashTableConfig<EC, D, S, H>,
}

impl<C, D, H> Default for DistributedHashTable<C, D, &NodeId, ConstTimeoutStrategy, H>
    where
        D: Serialize + DeserializeOwned,
        H: HashTable<&NodeId, D> + Expiring<C>,
{
    fn default() -> Self {
        Self {
            _c: Default::default(),
            state: Default::default(),
            config: Default::default(),
        }
    }
}

// todo implement default

impl<C, D, EC, S, H> DistributedHashTable<C, D, EC, S, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        D: Serialize + DeserializeOwned,
        H: HashTable<&NodeId, D, StoreErr=StoreErr::DataTypeErr> + Expiring<EC>,
        S: TimeoutStrategy<EC>
{
    fn handle_store_req(&mut self, context: C, req: ReqRspMessage<StoreReqData<D>>) -> Result<(), DHTError> {
        let res = self.config.hash_table.store(context.root_id(), req.data.data);
        let rsp = ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: StateSeqNr::from(0), // <-- todo what here
            data: StoreRspData {
                status: res
            },
            not_via: Default::default(),
            source_route: SourceRoute::from_reversed(req.source_route), // todo is this right?
            // maybe look into similar req -> send rsp occurrences
        };

        log::trace!(
            target: "distributed_hash_table",
            "Sending message: {:?}",
            rsp
        );

        if let Err(e) = context.message_sender_mut().send_message(rsp) {
            log::error!("Failed to send message: {}", e);
            return Err(DHTError(e)); // todo return more descriptive error
        }

        Ok(())
    }
}

impl<C, D, EC, S, H> DistributedHashTable<C, D, EC, S, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        D: Serialize + DeserializeOwned,
        H: HashTable<&NodeId, D, FetchErr=FetchErr::NotFoundErr> + Expiring<EC>,
        S: TimeoutStrategy<EC>
{
    fn handle_fetch_req(&mut self, context: C, req: ReqRspMessage<FetchReqData>) -> Result<(), DHTError> {
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
            target: "distributed_hash_table",
            "Sending message: {:?}",
            rsp
        );

        if let Err(e) = context.message_sender_mut().send_message(rsp) {
            log::error!("Failed to send message: {}", e);
            return Err(DHTError(e));
        }

        Ok(())
    }
}


impl<C, D, EC, S, H> EventHandler for DistributedHashTable<C, D, EC, S, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        D: Serialize + DeserializeOwned,
        H: HashTable<&NodeId, D, StoreErr=StoreErr::DataTypeErr, FetchErr=FetchErr::NotFoundErr> + Expiring<EC>,
        S: TimeoutStrategy<EC>
{
    type Context = C;
    type Error = DHTError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match event {
            // StoreReq
            UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _) => {
                self.handle_store_req(context, req)
            }
            UseCaseEvent::Message(ProtocolMessage::FetchRep(req), _) => {
                self.handle_fetch_req(context, req)
            }
            _ => Ok(())
        }
    }
}


impl<C, D, EC, S, H> UseCase for DistributedHashTable<C, D, EC, S, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        D: Serialize + DeserializeOwned,
        H: HashTable<&NodeId, D, StoreErr=StoreErr::DataTypeErr, FetchErr=FetchErr::NotFoundErr> + Expiring<EC>,
        S: TimeoutStrategy<EC>
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        self.state = ReactiveUseCaseState::default();
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
