//! Netlink [Connection](RtNetlinkConnection) managing for observing and collecting changes in the underlay.
//!
//! The main struct is the [UnderlayObserverConnection] which drives the progress
//! of the Netlink [Connection](RtNetlinkConnection).

use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::task::Poll;

use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use futures::{FutureExt, SinkExt, StreamExt};

use netlink_packet_core::{
    NetlinkHeader, NetlinkMessage, NetlinkPayload, NLM_F_DUMP, NLM_F_REQUEST,
};
use netlink_packet_route::link::LinkMessage;
use netlink_packet_route::{
    link::{LinkAttribute, LinkLayerType, State},
    RouteNetlinkMessage,
};
use netlink_proto::sys::AsyncSocket;
use netlink_proto::sys::SocketAddr;
use netlink_proto::Connection;
use netlink_proto::ConnectionHandle;
use rtnetlink::constants::RTMGRP_LINK;

use super::handle::UnderlayObserverHandleRequest;
use super::*;
use crate::domain::underlay::{Interface, InterfaceId, UnderlayNeighborUpdate};

type RtNetlinkReceiver = UnboundedReceiver<(NetlinkMessage<RouteNetlinkMessage>, SocketAddr)>;
/// [Connection] to Netlink which receives updates to [Interfaces](Interface) of the underlay.
pub type RtNetlinkConnection = Connection<RouteNetlinkMessage>;

struct UnderlayObserverInnerConnection {
    connection: RtNetlinkConnection,
}

impl UnderlayObserverInnerConnection {
    pub fn new(mut connection: RtNetlinkConnection) -> std::io::Result<Self> {
        // bind socket to receive link change events
        let socket = connection.socket_mut().socket_mut();
        socket.bind(&SocketAddr::new(0, RTMGRP_LINK))?;

        Ok(Self {
            connection,
            //netlink_handle,
        })
    }
}

impl Future for UnderlayObserverInnerConnection {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        log::trace!("Polling UnderlayConnection");
        // simple delegation to connection for now
        Pin::new(&mut self.get_mut().connection).poll(cx)
    }
}

/// Processing struct for handling underlay changes.
///
/// The [UnderlayObserverConnection] should usually be spawned in a thread using [tokio::spawn].
pub struct UnderlayObserverConnection {
    information_base: UnderlayInformationBase,

    connection: Option<UnderlayObserverInnerConnection>,

    // channels
    rt_messages: Option<UnboundedReceiver<NetlinkMessage<RouteNetlinkMessage>>>,
    updates_tx: Option<UnderlayNeighborUpdatesTx>,
    handle_rx: Option<UnboundedReceiver<UnderlayObserverHandleRequest>>,
}

impl UnderlayObserverConnection {
    pub(super) fn new(
        connection: RtNetlinkConnection,
        rt_handle: ConnectionHandle<RouteNetlinkMessage>,
        mut rt_messages: RtNetlinkReceiver,
        updates_tx: UnderlayNeighborUpdatesTx,
        handle_rx: UnboundedReceiver<UnderlayObserverHandleRequest>,
    ) -> std::io::Result<Self> {
        let (mut init_tx, mut init_rx) = unbounded();
        // get situation on startup
        tokio::spawn(async move {
            // Create the netlink message that requests the links to be dumped
            let mut nl_hdr = NetlinkHeader::default();
            nl_hdr.flags = NLM_F_DUMP | NLM_F_REQUEST;

            let msg = NetlinkMessage::new(
                nl_hdr,
                <NetlinkPayload<RouteNetlinkMessage>>::from(RouteNetlinkMessage::GetLink(
                    LinkMessage::default(),
                )),
            );

            // Send the request
            log::debug!("Sending initial request");
            let request = rt_handle.request(msg, SocketAddr::new(0, 0));
            let mut response = request.expect("request should complete succesfully");

            // Print all the messages received in response
            while let Some(message) = response.next().await {
                init_tx
                    .send(message)
                    .await
                    .expect("receiver should not have been dropped");
            }
        });

        // join initial and rt_messages channel
        let (mut messages_tx, messages_rx) = unbounded();
        tokio::spawn(async move {
            log::trace!("joining rt_messages and initial messages");

            // initial messages
            while let Some(message) = init_rx.next().await {
                messages_tx.send(message).await.unwrap();
            }

            log::trace!("initial messages finished");

            // rt_messages
            while let Some((message, _)) = rt_messages.next().await {
                messages_tx.send(message).await.unwrap();
            }
        });

        let connection = UnderlayObserverInnerConnection::new(connection)?;
        Ok(Self {
            connection: Some(connection),
            rt_messages: Some(messages_rx),
            updates_tx: Some(updates_tx),
            handle_rx: Some(handle_rx),
            information_base: Default::default(),
        })
    }

    fn poll_connection(&mut self, cx: &mut std::task::Context<'_>) {
        log::trace!("polling connection");
        if matches!(
            self.connection.as_mut().map(|c| c.poll_unpin(cx)),
            Some(Poll::Ready(_))
        ) {
            let _ = self.connection.take();
            log::trace!("connection closed");
        }
    }

    fn poll_messages(&mut self, cx: &mut std::task::Context<'_>) {
        let Some(rt_messages) = self.rt_messages.as_mut() else {
            return;
        };

        log::trace!("polling messages");
        match rt_messages.poll_next_unpin(cx) {
            Poll::Ready(Some(message)) => {
                match message.payload {
                    NetlinkPayload::Error(err_message) => {
                        log::error!("received an error message: {:?}", err_message);
                    }
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(message)) => {
                        //log::trace!("{message:?}");

                        if message.header.link_layer_type != LinkLayerType::Ether {
                            log::warn!("non ether link update detected: {message:?}");
                        }

                        let interface_id = message.header.index;
                        let interface_id: NonZeroU32 = interface_id.try_into().unwrap();
                        let interface_id = InterfaceId::from(interface_id);

                        let mut mac_addr = None;
                        let mut broadcast_addr = None;
                        let mut proto_down = false;
                        let mut link_state = None;

                        for attr in message.attributes.into_iter() {
                            match attr {
                                // TODO: maybe we need to react to changed attributes
                                LinkAttribute::Address(addr) => {
                                    let addr = addr.try_into().expect("MAC should fit");
                                    mac_addr.replace(addr);
                                }
                                LinkAttribute::Broadcast(addr) => {
                                    let addr = addr.try_into().expect("MAC should fit");
                                    broadcast_addr.replace(addr);
                                }
                                // TODO: LinkAttribute::Mode(mode) => todo!("Handle link mode"),
                                LinkAttribute::Carrier(1) => {}
                                LinkAttribute::Carrier(carrier) => {
                                    log::warn!("Ignoring carrier {} != 1", carrier)
                                }
                                LinkAttribute::ProtoDown(0) => {
                                    proto_down = false;
                                }
                                LinkAttribute::ProtoDown(_) => {
                                    proto_down = true;
                                }
                                LinkAttribute::OperState(state) => {
                                    link_state.replace(state);
                                }
                                _ => {}
                            }
                        }

                        log::trace!("ProtoDown: {proto_down}");
                        log::trace!("State: {link_state:?}");

                        let if_up =
                            !proto_down && link_state.is_some_and(|state| state != State::Down);

                        if if_up {
                            let interface = Interface::new(
                                interface_id,
                                mac_addr.expect("MAC address should be supplied by rtnetlink"),
                                broadcast_addr
                                    .expect("MAC address should be supplied by rtnetlink"),
                            );
                            log::debug!("Interface up: {:?}", interface);

                            self.information_base.interface_up(interface);
                            if self
                                .updates_tx
                                .as_mut()
                                .unwrap()
                                .unbounded_send(UnderlayNeighborUpdate::InterfaceUp(interface_id))
                                .is_err()
                            {
                                let _ = self.updates_tx.take();
                            }
                        } else {
                            log::debug!("Interface down: {}", interface_id);

                            let Some(affected) =
                                self.information_base.interface_down(&interface_id)
                            else {
                                // no neighbors affected
                                log::warn!("Interface {interface_id} was not registered as up yet");
                                return;
                            };

                            if self
                                .updates_tx
                                .as_mut()
                                .unwrap()
                                .unbounded_send(UnderlayNeighborUpdate::InterfaceDown(interface_id))
                                .is_err()
                            {
                                let _ = self.updates_tx.take();
                                return;
                            }

                            // queue updates of affected neighbors
                            for affected in affected {
                                if self
                                    .updates_tx
                                    .as_mut()
                                    .unwrap()
                                    .unbounded_send(UnderlayNeighborUpdate::UnderlayNeighborDown(
                                        affected,
                                    ))
                                    .is_err()
                                {
                                    let _ = self.updates_tx.take();
                                    break;
                                }
                            }
                        }
                    }

                    // ignore other messages but keep listening for new messages
                    _ => {}
                }
            }
            Poll::Ready(None) => {
                log::trace!("closed rt_messages");
                let _ = self.rt_messages.take();
            }
            Poll::Pending => {}
        }
    }

    fn poll_handle(&mut self, cx: &mut std::task::Context<'_>) {
        let Some(handle_rx) = self.handle_rx.as_mut() else {
            return;
        };

        log::trace!("polling UnderlayObserverHandle");
        match handle_rx.poll_next_unpin(cx) {
            Poll::Ready(Some(UnderlayObserverHandleRequest::GetInformation {
                ulnid,
                response,
            })) => {
                let info = self.information_base.get_information(&ulnid);
                response
                    .send(info)
                    .expect("receiver should not get dropped");
            }
            Poll::Ready(Some(UnderlayObserverHandleRequest::RegisterUnderlayNeighbor {
                interface_id,
                ll_ipv6,
                response,
            })) => {
                let reg_info = self
                    .information_base
                    .register_neighbor(interface_id, ll_ipv6);
                if reg_info.is_ok()
                    && self
                        .updates_tx
                        .as_mut()
                        .map(|updates_tx| {
                            updates_tx.unbounded_send(UnderlayNeighborUpdate::UnderlayNeighborUp(
                                *reg_info.as_ref().unwrap(),
                            ))
                        })
                        .is_some_and(|s| s.is_err())
                {
                    let _ = self.updates_tx.take();
                }
                response
                    .send(reg_info)
                    .expect("receiver should not get dropped");
            }
            Poll::Ready(Some(UnderlayObserverHandleRequest::UnregisterUnderlayNeighbor {
                ulnid,
            })) => {
                let existed = self.information_base.unregister_neighbor(&ulnid).is_some();
                if existed
                    && self
                        .updates_tx
                        .as_mut()
                        .map(|updates_tx| {
                            updates_tx
                                .unbounded_send(UnderlayNeighborUpdate::UnderlayNeighborDown(ulnid))
                        })
                        .is_some_and(|s| s.is_err())
                {
                    let _ = self.updates_tx.take();
                }
            }
            Poll::Ready(Some(UnderlayObserverHandleRequest::GetAvailable { response })) => {
                let interfaces = self.information_base.get_available().copied().collect();
                response
                    .send(interfaces)
                    .expect("receiver should not get dropped");
            }
            Poll::Ready(None) => {
                let _ = self.handle_rx.take();
            }
            Poll::Pending => {}
        }
    }
}

impl Future for UnderlayObserverConnection {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let pinned = self.get_mut();

        pinned.poll_connection(cx);
        pinned.poll_messages(cx);
        pinned.poll_handle(cx);

        if pinned.updates_tx.is_none()
            || (pinned.rt_messages.is_none() && pinned.handle_rx.is_none())
        {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

// TODO: unit tests
