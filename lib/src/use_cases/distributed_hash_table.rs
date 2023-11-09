use std::error::Error;
use std::fmt::{Display, Formatter};
use std::time::Instant;
use core::time::Duration;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use crate::context::UseCaseContext;
use crate::domain::dht::{DHTData, Expiring, ExpiringHashTable, HashTable, TimeoutStrategy};
use crate::domain::NodeId;
use crate::messaging::ProtocolMessageSender;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig<C, D, S, H>
where
    H: HashTable<NodeId, D> + Expiring<C>, // not happy about this
    S: TimeoutStrategy<C>
{
    strategy: S, // not happy about this either
    hash_table: H,
    collect_interval: Duration,
    send_timeout: Duration,
}
// todo implement default

struct ConstTimeoutStrategy {
    expire_after: Duration
}

impl<C> TimeoutStrategy<C> for ConstTimeoutStrategy {
    fn is_timed_out(&self, context: C, time: Instant) -> bool {
        Instant::now().duration_since(time) >= self.expire_after
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum DHTError {
    DHTError
}

impl Display for DHTError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl Error for DHTError {}

pub enum DHTState {
    Initialized,
    Running,
    Error,
}

impl UseCaseState for DHTState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

pub struct DistributedHashTable<C, D, EC, S, H>
where
    D: Serialize + DeserializeOwned,
    H: HashTable<NodeId, D> + Expiring<C>,
    S: TimeoutStrategy<C>
{
    //_c: PhantomData<C>,
    state: DHTState,
    config: DistributedHashTableConfig<EC, D, S, H>,
}
// todo implement default

impl DistributedHashTable<UseCaseContext, DHTData, NodeId, ConstTimeoutStrategy<>, ExpiringHashTable<NodeId, DHTData>> {

}

impl<C, D, S, H> EventHandler for DistributedHashTable<C, D, NodeId, S, H>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime

{
    type Context = C;
    type Error = DHTError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        todo!()
    }
}

impl<C, D, S, H> UseCase for DistributedHashTable<C, D, NodeId, S, H>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime
{
    type State = DHTState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        self.state = DHTState::Running;
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
