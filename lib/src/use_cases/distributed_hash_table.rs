use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use core::time::Duration;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::context::UseCaseContext;
use crate::domain::NodeId;
use crate::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr, FetchRspData, StoreResult, StoreRspData};

use crate::domain::dht::TimedValue;
use crate::domain::dht::expiring::Expiring;
use crate::domain::dht::hash_table::expiring_hash_table::ExpiringHashTable;
use crate::domain::dht::hash_table::LocalHashTable;
use crate::domain::dht::strategies::fetch_strategy::PermissionlessFetchStrategy;
use crate::domain::dht::strategies::insert_strategy::PermissionlessInsertStrategy;
use crate::domain::dht::strategies::timeout_strategy::{ConstTimeoutStrategy, TimeoutStrategy};

use crate::messaging::error::SenderError;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};

pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_secs(60);

pub type HashTableSingle = TimedValue<Arc<[u8]>>;
pub type HashTableData = HashSet<HashTableSingle>;
pub type DefaultExpiringHashTable = ExpiringHashTable<
    NodeId,
    HashTableData,
    PermissionlessInsertStrategy,
    PermissionlessFetchStrategy,
    ConstTimeoutStrategy<NodeId, Arc<[u8]>>,
>;

impl<TS> Expiring for ExpiringHashTable<
    NodeId,
    HashTableData,
    PermissionlessInsertStrategy,
    PermissionlessFetchStrategy,
    TS,
> where
    TS: TimeoutStrategy<Context=NodeId, Expirable=HashTableSingle>
{
    type Context = ();
    type Result = ();

    fn expire(&mut self, context: &Self::Context) -> Self::Result {
        for (h, set) in self.map.iter_mut() {
            set.retain(|tv| !self.timeout_strategy.has_timed_out(h, tv));
        }
    }
}


#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig<H>
{
    hash_table: H,
    collect_interval: Duration,
}

impl Default for DistributedHashTableConfig<DefaultExpiringHashTable> {
    fn default() -> Self {
        let hash_table = ExpiringHashTable::new(
            PermissionlessInsertStrategy::default(),
            PermissionlessFetchStrategy::default(),
            ConstTimeoutStrategy::default(),
        );

        Self {
            hash_table,
            collect_interval: DEFAULT_COLLECT_INTERVAL,
        }
    }
}

#[derive(Debug)]
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

pub struct DistributedHashTable<C, H>
{
    _c: PhantomData<C>,
    state: DHTState,
    config: DistributedHashTableConfig<H>,
}

impl UseCaseState for DHTState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, H> DistributedHashTable<C, H>
{
    pub fn new(config: DistributedHashTableConfig<H>) -> Self {
        Self {
            _c: PhantomData::default(),
            state: DHTState::default(),
            config,
        }
    }
}

impl<C, H, RS> EventHandler for DistributedHashTable<C, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        H: LocalHashTable<NodeId, DefaultLHTInput, DefaultLHTOutput, StoreRes=StoreResult, FetchErr=FetchErr> + Expiring<Context=(), Result=RS>
{
    type Context = C;
    type Error = DHTError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                let res = self.config.hash_table.store(req.data.handle, req.data.data);
                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data: StoreRspData {
                        status: res
                    },
                    not_via: context.not_via().clone(),
                    source_route: SourceRoute::from_reversed(req.source_route),
                };

                log::trace!(
                    target: "dht",
                    "Sending message: {:?}",
                    rsp
                );

                if let Err(e) = context.message_sender_mut().send_message(ProtocolMessage::StoreRsp(rsp)) {
                    log::error!("Failed to send message: {}", e);
                    return Err(DHTError::DHTSendError(e));
                }
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchRep(req), _), _) => {
                let fetch_res = self.config.hash_table.fetch(&req.data.handle);
                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data: FetchRspData {
                        data: fetch_res,
                    },
                    not_via: context.not_via().clone(),
                    source_route: SourceRoute::from_reversed(req.source_route),
                };

                log::trace!(
                    target: "dht",
                    "Sending message: {:?}",
                    rsp
                );

                if let Err(e) = context.message_sender_mut().send_message(ProtocolMessage::FetchRsp(rsp)) {
                    log::error!("Failed to send message: {}", e);
                    return Err(DHTError::DHTSendError(e));
                }
            }
            (UseCaseEvent::Timer(id), DHTState::Running(our_timer_id)) => {
                if &id == our_timer_id {
                    self.config.hash_table.expire(&());
                }

                let timer_id = context
                    .runtime()
                    .register_timer(self.config.collect_interval);

                self.state = DHTState::Running(timer_id);
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, H, RS> UseCase for DistributedHashTable<C, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        H: LocalHashTable<NodeId, DefaultLHTInput, DefaultLHTOutput, StoreRes=StoreResult, FetchErr=FetchErr> + Expiring<Context=(), Result=RS>
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
