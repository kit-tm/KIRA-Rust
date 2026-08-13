use std::{
    collections::HashMap,
    fmt::Debug,
    num::{
        NonZeroU8,
        NonZeroUsize,
    },
    ops::Deref,
    time::Duration,
};

use derive_more::From;

use crate::{
    domain::{
        Contact,
        NodeId,
        Path,
        RoutingTable,
        ULNTable,
        UnderlayNeighborId,
    },
    messaging::{
        CommonHeader,
        Nonce,
        ProtocolMessage,
        ProtocolMessageKind,
        ReqRspMessage,
        dht::{
            FetchReqData,
            LHTInput,
            StoreReqData,
        },
        source_route::SourceRoute,
    },
    use_cases::{
        BroadcastableUseCaseEvent,
        UseCaseContext,
        UseCaseRuntime,
        inject_messages::errors::InjectMessageError,
    },
};

mod hash_table;
pub use hash_table::*;

/// Default timeout duration of RPCs by the DHT use cases.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default, PartialEq, Eq, From, Clone, Copy)]
/// The redundancy factor ensures that data is stored at multiple locations to prevent loss if nodes
/// go offline.
///
/// The replicas are stored at the *k* key-wise closest nodes of.
/// You can resolve the concrete *k* using [RedundancyFactor::resolve].
///
/// The redundancy factor *includes* the key-wise closest node.
pub enum RedundancyFactor {
    #[default]
    BucketSize,
    #[from]
    Fixed(NonZeroUsize),
}

impl RedundancyFactor {
    /// Resolve the concrete [RedundancyFactor] factor.
    ///
    /// # Examples
    ///
    /// ```
    /// # use kira_r2kad::domain::dht::RedundancyFactor;
    ///
    /// let bucket_size = 20;
    /// let fixed = 42;
    ///
    /// // When set to BucketSize it should return the provided bucket size.
    /// let bucket_redundancy = RedundancyFactor::BucketSize;
    /// assert_eq!(bucket_redundancy.resolve(bucket_size).get(), bucket_size);
    /// // The default RedundancyFactor is BucketSize.
    /// assert_eq!(RedundancyFactor::default().resolve(bucket_size).get(), bucket_size);
    ///
    /// // When set to a fixed value it should return the fixed value.
    /// let fixed_redundancy = RedundancyFactor::Fixed(fixed.try_into().unwrap());
    /// assert_eq!(fixed_redundancy.resolve(bucket_size).get(), fixed);
    /// ```
    pub fn resolve(&self, bucket_size: usize) -> NonZeroUsize {
        match self {
            Self::BucketSize => bucket_size.try_into().expect("bucket size > 0"),
            Self::Fixed(redundancy) => *redundancy,
        }
    }
}

// TODO: move the following into the DistributedHashTableInjector UseCase

/// Construct a protocol message that is routed to its destination
/// by key-based routing.
///
/// The initial overlay hop is determined using proximity routing.
pub(crate) fn construct_req_rsp_msg_kbr<C, T, const BUCKET_SIZE: usize>(
    context: &C,
    pdutype: ProtocolMessageKind,
    nonce: Nonce,
    overlay_destination: NodeId,
    data: T,
) -> ReqRspMessage<T>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable,
    T: Debug,
{
    // TODO: support other shared_prefix_grouping via config
    let closest_node = context
        .routing_table()
        .next_hop(&overlay_destination, NonZeroU8::MIN)
        .expect("Shared Prefix Grouping should be valid");

    let path = if let Some(path) = closest_node.and_then(Contact::into_path) {
        path
    } else {
        tracing::warn!(target: "distributed_hash_table", "Node is isolated!");
        // send message via loopback because of the isolation we are the closest node
        Path::from(*context.root_id())
    };

    let source_route = SourceRoute::new(*context.root_id(), path);

    ReqRspMessage {
        common_header: CommonHeader::new(
            pdutype,
            *context.root_id(),
            overlay_destination,
            Some(nonce.into()),
            Some(u32::from(*context.uln_table().state_seq_nr())),
            context.uln_table().size(),
        ),
        not_via: None,
        data,
        source_route,
    }
}

/// Send a StoreReq that is routed by key-based routing.
pub(crate) fn send_store_req_kbr<C, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: StoreReqData<LHTInput>,
    destination: NodeId,
) where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let message = construct_req_rsp_msg_kbr(
        context,
        ProtocolMessageKind::StoreReq,
        nonce,
        destination,
        data,
    );

    tracing::trace!(
        target: "distributed_hash_table",
        key = %message.data.handle,
        %destination,
        source_route = ?message.source_route,
        "Sending StoreReq",
    );

    let message = ProtocolMessage::StoreReq(message);
    if message.current_hop().unwrap() == context.root_id() {
        context
            .runtime()
            .broadcast_event(BroadcastableUseCaseEvent::Message(message));
        return;
    }

    context
        .runtime()
        .send_message(message, context.uln_table().deref(), context.root_id());
}

/// Send a StoreReq.
pub(crate) fn send_store_req<C, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: StoreReqData<LHTInput>,
    source_route: SourceRoute,
) where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let destination = *source_route.destination();

    let message = ReqRspMessage {
        common_header: CommonHeader::new(
            ProtocolMessageKind::StoreReq,
            *context.root_id(),
            destination,
            Some(nonce.into()),
            Some(u32::from(*context.uln_table().state_seq_nr())),
            context.uln_table().size(),
        ),
        not_via: None,
        data,
        source_route,
    };

    tracing::trace!(
        target: "distributed_hash_table",
        key = %message.data.handle,
        %destination,
        source_route = ?message.source_route,
        "Sending StoreReq",
    );

    let message = ProtocolMessage::StoreReq(message);
    if message.current_hop().unwrap() == context.root_id() {
        context
            .runtime()
            .broadcast_event(BroadcastableUseCaseEvent::Message(message));
        return;
    }

    context
        .runtime()
        .send_message(message, context.uln_table().deref(), context.root_id());
}

/// Send a FetchReq that is routed by key-based routing.
pub(crate) fn send_fetch_req<C, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: FetchReqData,
    destination: NodeId,
) -> Result<(), InjectMessageError>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let message = construct_req_rsp_msg_kbr(
        context,
        ProtocolMessageKind::FetchReq,
        nonce,
        destination,
        data,
    );

    tracing::trace!(
        target: "distributed_hash_table",
        key = %message.data.handle,
        %destination,
        source_route = ?message.source_route,
        "Sending FetchReq",
    );

    let message = ProtocolMessage::FetchReq(message);
    // TODO: remove duplicated code
    if message.current_hop().unwrap() == context.root_id() {
        context
            .runtime()
            .broadcast_event(BroadcastableUseCaseEvent::Message(message));
        return Ok(());
    }

    context
        .runtime()
        .send_message(message, context.uln_table().deref(), context.root_id());
    Ok(())
}
