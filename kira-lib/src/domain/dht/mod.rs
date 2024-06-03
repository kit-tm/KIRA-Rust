use std::fmt::Debug;

use crate::context::UseCaseContext;
use crate::domain::NetworkInterface;
use crate::domain::NodeId;
use crate::domain::PNTable;
use crate::domain::Path;
use crate::domain::RoutingTable;
use crate::messaging::dht::DefaultLHTInput;
use crate::messaging::dht::FetchReqData;
use crate::messaging::dht::StoreReqData;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::Nonce;
use crate::messaging::ProtocolMessage;
use crate::messaging::ProtocolMessageSender;
use crate::messaging::ReqRspMessage;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::inject_messages::errors::InjectMessageError;
use crate::use_cases::UseCaseEvent;

mod expiring;
mod timed;

pub use expiring::Expiring;
pub use timed::*;

pub mod hash_table;
pub mod strategies;

pub(crate) fn construct_req_rsp_msg<C, T, D, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    overlay_destination: &NodeId,
    data: T,
) -> ReqRspMessage<T>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime<SendError = D>,
    C::PhysicalNeighborTable: PNTable,
    T: Debug,
    D: Debug,
{
    // todo support other shared_prefix_grouping via config
    let closest_node = context
        .routing_table()
        .next_hop(overlay_destination, 20, 1)
        .expect("Shared Prefix Grouping should be valid");

    // we are the closest => loopback
    let path = if closest_node.is_none() {
        log::warn!(target: "distributed_hash_table_injector", "Node is isolated!");

        Path::from(context.root_id().clone())
    } else {
        closest_node.unwrap().path().clone()
    };

    let source_route = SourceRoute::new(context.root_id().clone(), path);

    ReqRspMessage {
        source_state_seq_nr: *context.pn_table().state_seq_nr(),
        not_via: context.not_via().clone(),
        data,
        nonce,
        source_route,
    }
}

pub(crate) fn send_store_req<C, D, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: StoreReqData<DefaultLHTInput>,
) -> Result<(), InjectMessageError>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime<SendError = D>,
    C::PhysicalNeighborTable: PNTable,
    D: Debug,
{
    let message = construct_req_rsp_msg(context, nonce, &data.handle.clone(), data);

    log::trace!(target: "distributed_hash_table_injector", "Sending StoreReq from {} with destination {}",
        message.source_route.source(),
        message.data.handle
    );

    let message = ProtocolMessage::StoreReq(message);
    if message.destination().unwrap() == context.root_id() {
        return context.runtime().broadcast_local_event(UseCaseEvent::Message(message, NetworkInterface::loopback())).map_err(|e| {
            log::error!(target: "distributed_hash_table_injector", "Failed to send triggered message: {:?}", e);
            InjectMessageError::SendFailed
        });
    }

    context.message_sender_mut().send_message(message).map_err(|e| {
        log::error!(target: "distributed_hash_table_injector", "Failed to send triggered message: {:?}", e);
        InjectMessageError::SendFailed
    })
}

pub(crate) fn send_fetch_req<C, D, const BUCKET_SIZE: usize>(
    context: &C,
    nonce: Nonce,
    data: FetchReqData,
) -> Result<(), InjectMessageError>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime<SendError = D>,
    C::PhysicalNeighborTable: PNTable,
    D: Debug,
{
    let message = construct_req_rsp_msg(context, nonce, &data.handle.clone(), data);

    log::trace!(target: "distributed_hash_table_injector", "Sending FetchReq from {} with destination {}",
        message.source_route.source(),
        message.data.handle
    );

    let message = ProtocolMessage::FetchReq(message);
    // TODO remove duplicated code
    if message.destination().unwrap() == context.root_id() {
        return context.runtime().broadcast_local_event(UseCaseEvent::Message(message, NetworkInterface::loopback())).map_err(|e| {
            log::error!(target: "distributed_hash_table_injector", "Failed to send triggered message: {:?}", e);
            InjectMessageError::SendFailed
        });
    }

    context.message_sender_mut().send_message(message).map_err(|e| {
        log::error!(target: "distributed_hash_table_injector", "Failed to send triggered message: {:?}", e);
        InjectMessageError::SendFailed
    })
}



