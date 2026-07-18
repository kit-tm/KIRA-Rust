use std::{collections::HashMap, fmt::Debug, num::NonZeroU8, ops::Deref};

use crate::{
    domain::{NodeId, Path, RoutingTable, ULNTable, UnderlayNeighborId},
    messaging::{
        CommonHeader, Nonce, ProtocolMessageKind, ReqRspMessage,
        dht::{DefaultLHTInput, FetchReqData, StoreReqData},
        source_route::SourceRoute,
    },
    use_cases::{UseCaseContext, UseCaseRuntime, inject_messages::errors::InjectMessageError},
};

mod expiring;
mod timed;

pub use expiring::Expiring;
pub use timed::*;

pub mod hash_table;
pub mod strategies;

pub(crate) fn construct_req_rsp_msg<C, T, const BUCKET_SIZE: usize>(
    context: &C,
    pdutype: ProtocolMessageKind,
    nonce: Nonce,
    overlay_destination: &NodeId,
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
        .next_hop(overlay_destination, NonZeroU8::MIN)
        .expect("Shared Prefix Grouping should be valid");

    let path = if let Some(closest_node) = closest_node
        && closest_node.path().is_some()
    {
        closest_node.path().unwrap().clone()
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
            *overlay_destination,
            Some(nonce.into()),
            Some(u32::from(*context.uln_table().state_seq_nr())),
            context.uln_table().size(),
        ),
        not_via: None,
        data,
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
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let message = construct_req_rsp_msg(
        context,
        ProtocolMessageKind::StoreReq,
        nonce,
        &data.handle.clone(),
        data,
    );

    log::trace!(target: "distributed_hash_table_injector", "Sending StoreReq from {} with destination {}",
        message.source_route.source(),
        message.data.handle
    );

    context
        .runtime()
        .send_message(message, context.uln_table().deref(), context.root_id());
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
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    let message = construct_req_rsp_msg(
        context,
        ProtocolMessageKind::FetchReq,
        nonce,
        &data.handle.clone(),
        data,
    );

    log::trace!(target: "distributed_hash_table_injector", "Sending FetchReq from {} with destination {}",
        message.source_route.source(),
        message.data.handle
    );

    context
        .runtime()
        .send_message(message, context.uln_table().deref(), context.root_id());
    Ok(())
}
