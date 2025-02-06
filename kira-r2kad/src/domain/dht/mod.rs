use std::{collections::HashMap, fmt::Debug, ops::Deref};

use crate::{
    domain::{NodeId, Path, RoutingTable, UNTable, UnderlayNeighborId},
    messaging::{
        dht::{DefaultLHTInput, FetchReqData, StoreReqData},
        source_route::SourceRoute,
        Nonce, ProtocolMessage, ReqRspMessage,
    },
    use_cases::{
        inject_messages::errors::InjectMessageError, BroadcastableUseCaseEvent, UseCaseContext,
        UseCaseRuntime,
    },
};

mod expiring;
mod timed;

pub use expiring::Expiring;
pub use timed::*;

pub mod hash_table;
pub mod strategies;

pub(crate) fn construct_req_rsp_msg<C, T, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    overlay_destination: &NodeId,
    data: T,
) -> ReqRspMessage<T>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: UNTable,
    T: Debug,
{
    // todo support other shared_prefix_grouping via config
    let closest_node = context
        .routing_table()
        .next_hop(overlay_destination, 20, 1)
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
        source_state_seq_nr: *context.un_table().state_seq_nr(),
        not_via: context.not_via().clone(),
        data,
        nonce,
        source_route,
    }
}

pub(crate) fn send_store_req<C, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: StoreReqData<DefaultLHTInput>,
) where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let message = construct_req_rsp_msg(context, nonce, &data.handle.clone(), data);

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
        .send_message(message, context.un_table().deref());
}

pub(crate) fn send_fetch_req<C, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: FetchReqData,
) -> Result<(), InjectMessageError>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let message = construct_req_rsp_msg(context, nonce, &data.handle.clone(), data);

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
        .send_message(message, context.un_table().deref());
    Ok(())
}
