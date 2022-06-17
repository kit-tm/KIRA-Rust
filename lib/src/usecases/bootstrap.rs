use std::{
    error::Error,
    fmt::Display,
    time::{Duration, Instant},
};

use crate::{
    domain::{Interface, NeighborTable, NodeId},
    messaging::{
        FindNodeReqData, FindNodeRspData, HelloMessage, Message, MessageReceiver, MessageSender,
        Nonce, PNDiscRspData, ReqRspMessage,
    },
};

#[derive(Debug)]
pub enum BootstrapError {
    SendError,
}

impl Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Failed to send a message over the given MessageSender"),
        }
    }
}

impl Error for BootstrapError {}

pub struct BootstrapConfig {
    max_neighbor_response_duration: Duration,
}

pub trait Bootstrap<const ID_SIZE: usize> {
    fn start(&mut self, config: BootstrapConfig) -> Result<(), BootstrapError>;
}

pub struct BootstrapUseCase<'a, NT, MS, MR, const ID_SIZE: usize> {
    root_id: NodeId<ID_SIZE>,
    neighbor_table: &'a NT,
    sender: &'a mut MS,
    receiver: &'a mut MR,
}

impl<'a, NT, MS, MR, const ID_SIZE: usize> BootstrapUseCase<'a, NT, MS, MR, ID_SIZE> {
    pub fn new(
        root_id: NodeId<ID_SIZE>,
        neighbor_table: &'a NT,
        sender: &'a mut MS,
        receiver: &'a mut MR,
    ) -> Self {
        Self {
            root_id,
            neighbor_table,
            sender,
            receiver,
        }
    }
}

impl<'a, NT, MS, MR, const ID_SIZE: usize> Bootstrap<ID_SIZE>
    for BootstrapUseCase<'a, NT, MS, MR, ID_SIZE>
where
    NT: NeighborTable<ID_SIZE>,
    for<'b> &'b NT: IntoIterator<Item = (&'b NodeId<ID_SIZE>, &'b Interface)>,
    MS: MessageSender<ID_SIZE>,
    MR: MessageReceiver<ID_SIZE>,
{
    fn start(&mut self, config: BootstrapConfig) -> Result<(), BootstrapError> {
        // Although the UseCase contains generating the NodeId
        // it's required to pass this before as the RoutingTable
        // and others require the root NodeId before Bootstrap is started.
        assert!(self.root_id != NodeId::zero());

        // Send hello to all links
        if let Err(e) = self.sender.send(HelloMessage {
            source: self.root_id.clone(),
            destination: NodeId::zero(),
        }) {
            log::error!("MessageSender failed: {}", e);
            return Err(BootstrapError::SendError);
        }

        // TODO: Wait for Responses or timeout
        let mut pn_disc_req = Vec::new();

        // DRY Adding to vec
        let mut filter_then_add = |message: Message<ID_SIZE>| {
            // Only collect bidirectional connectivity requests
            if let Message::PNDiscReq(data) = message {
                pn_disc_req.push(data);
            }
        };

        // Measure time to abort after timeout was reached
        let start = Instant::now();
        let mut timeout_duration = config.max_neighbor_response_duration;
        while let Ok(Some(message)) = self.receiver.recv_timeout(Some(timeout_duration)) {
            // Only collect bidirectional connectivity requests
            filter_then_add(message);
            // Read all if some messages are received as batch
            // Possibly reduces calculation of elapsed time
            while let Ok(Some(message)) = self.receiver.try_recv() {
                filter_then_add(message);
            }

            // Finish if tiimeout was reached
            let elapsed = start.elapsed();
            if elapsed >= config.max_neighbor_response_duration {
                break;
            }
            // Otherwise calc new read timeout
            timeout_duration = config.max_neighbor_response_duration - elapsed;
        }

        // Send PNDiscRsp to all neighbors discovered.
        // It's assumed, that all Neighbors answered with a PNDiscReq
        // after receiving the Hello Message.
        // The contact info of these PNDiscReq are added to the NeighborTable.
        // It can't be assumed, that they are in the RoutingTable
        // as the used InsertionStrategy decides that.
        let neighbors = self
            .neighbor_table
            .into_iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        while let Some(ReqRspMessage { id, source, .. }) = pn_disc_req.pop() {
            if let Err(e) = self.sender.send(ReqRspMessage {
                id,
                source: self.root_id.clone(),
                destination: source,
                data: PNDiscRspData {
                    contacts: neighbors.clone(),
                },
            }) {
                log::error!("MessageSender failed: {}", e);
                return Err(BootstrapError::SendError);
            }
        }

        // TODO: Send QueryRouteReqs and wait for responses

        let find_node_id_nonce = Nonce::random();
        if let Err(e) = self.sender.send(ReqRspMessage {
            id: find_node_id_nonce.clone(),
            source: self.root_id.clone(),
            destination: self.root_id.clone(),
            data: FindNodeReqData {},
        }) {
            log::error!("MessageSender failed: {}", e);
            return Err(BootstrapError::SendError);
        }

        // No timeout is used here as the FindNodeReq is assmued to
        // either return a FindNodeRsp or an Error.
        while let Some(message) = self.receiver.recv() {
            match message {
                Message::FindNodeRsp(ReqRspMessage {
                    id,
                    data: FindNodeRspData { .. },
                    ..
                }) => {
                    if id == find_node_id_nonce {
                        break;
                    }
                }
                Message::Error(ReqRspMessage { id, .. }) => {
                    if id == find_node_id_nonce {
                        log::warn!("Join FindNodeReq returned an Error Message");
                        break;
                    }
                }
                _ => continue,
            }
        }

        Ok(())
    }
}
