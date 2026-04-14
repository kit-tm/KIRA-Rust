use std::{collections::HashMap, fmt::Debug, num::NonZeroU8, ops::Deref};

use crate::{
    domain::{NodeId, NotVia, Path, RoutingTable, ULNTable, UnderlayNeighborId},
    messaging::{
        CommonHeader, Nonce, ProtocolMessage, ProtocolMessageKind, ReqRspMessage,
        dht::{DefaultLHTInput, FetchReqData, StoreReqData},
        source_route::SourceRoute,
    },
    use_cases::{
        BroadcastableUseCaseEvent, UseCaseContext, UseCaseRuntime,
        inject_messages::errors::InjectMessageError,
    },
};

mod expiring;
mod timed;

pub use expiring::Expiring;
pub use timed::*;

pub mod hash_table;
pub mod strategies;

/// Construct a protocol message that is routed to its destination
/// by key-based routing.
///
/// The initial overlay hop is determined using proximity routing.
pub(crate) fn construct_req_rsp_msg<C, T, const BUCKET_SIZE: usize>(
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

    let path = if let Some(closest_node) = closest_node {
        closest_node.path().clone()
    } else {
        // we are the closest => loopback
        log::warn!(target: "distributed_hash_table_injector", "Node is isolated!");

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
        not_via: context.not_via_state().iter().map(NotVia::from).collect(),
        data,
        source_route,
    }
}

/// Send a StoreReq that is routed by key-based routing.
pub(crate) fn send_store_req<C, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: StoreReqData<DefaultLHTInput>,
    destination: NodeId,
) where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let message = construct_req_rsp_msg(
        context,
        ProtocolMessageKind::StoreReq,
        nonce,
        destination,
        data,
    );

    log::trace!(target: "distributed_hash_table_injector", "Sending StoreReq from {} with destination {}",
        message.source_route.source(),
        message.data.handle
    );

    let message = ProtocolMessage::StoreReq(message);
    if message.destination().unwrap() == context.root_id() {
        context
            .runtime()
            .broadcast_event(BroadcastableUseCaseEvent::Message(message));
        return;
    }

    context
        .runtime()
        .send_message(message, context.uln_table().deref());
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
    let message = construct_req_rsp_msg(
        context,
        ProtocolMessageKind::FetchReq,
        nonce,
        destination,
        data,
    );

    log::trace!(target: "distributed_hash_table_injector", "Sending FetchReq from {} with destination {}",
        message.source_route.source(),
        message.data.handle
    );

    let message = ProtocolMessage::FetchReq(message);
    // TODO: remove duplicated code
    if message.destination().unwrap() == context.root_id() {
        context
            .runtime()
            .broadcast_event(BroadcastableUseCaseEvent::Message(message));
        return Ok(());
    }

    context
        .runtime()
        .send_message(message, context.uln_table().deref());
    Ok(())
}
