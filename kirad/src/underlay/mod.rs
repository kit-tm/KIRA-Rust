//! Interaction with the underlay of the node instance.

use kira_lib::domain::UnderlayNeighborId;
use kira_lib::domain::UnderlayNeighborUpdate;
use kira_lib::messaging::ProtocolMessage;
use netlink_proto::new_connection;
use netlink_proto::sys::AsyncSocket;
use netlink_proto::sys::SocketAddr;
use netlink_proto::sys::protocols::NETLINK_ROUTE;
use rtnetlink::constants::RTMGRP_LINK;
use tokio::sync::RwLock;

pub type UnderlaySender = Sender<UnderlayNeighborUpdate>;

pub struct UnderlayInformationBase {}

pub struct UnderlayObserver {
    tx: UnderlaySender,
    information: Arc<RwLock<UnderlayInformationBase>>,
}

impl UnderlayObserver {
    pub fn new(
        tx: UnderlaySender,
        information_base: &Arc<RwLock<UnderlayInformationBase>>,
    ) -> Self {
        let infomration = information_base.clone();

        let Ok((conn, mut handle, mut messages)) = new_connection(NETLINK_ROUTE) else {
            panic!("Failed to open Netlink socket to observe link changes!");
        };

        // bind socket to receive link change events
        let mut socket = conn.socket_mut().socket_mut();
        socket.bind(&SocketAddr::new(0, RTMGRP_LINK));

        // poll netlink socket in the background
        tokio::spawn(conn);

        Self { tx, information }
    }
}
