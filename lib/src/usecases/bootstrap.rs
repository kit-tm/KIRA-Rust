use std::{error::Error, fmt::Display, marker::PhantomData, time::Duration};

use crate::{
    domain::{DiscoveryTable, Interface, NeighborTable, NodeId, RoutingTable},
    messaging::{
        FindNodeReqData, HelloMessage, Message, MessageReceiver, MessageSender, Nonce,
        PNDiscRspData, ReqRspMessage,
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

pub struct BootstrapUseCase<'a, RT, NT, DC, MS, MR, const ID_SIZE: usize, const BUCKET_SIZE: usize>
{
    routing_table: RT,
    neighbor_table: NT,
    discovery_cache: DC,
    sender: MS,
    receiver: MR,
    _lt: PhantomData<&'a RT>,
}

impl<'a, RT, NT, DC, MS, MR, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    BootstrapUseCase<'a, RT, NT, DC, MS, MR, ID_SIZE, BUCKET_SIZE>
{
    pub fn new(
        routing_table: RT,
        neighbor_table: NT,
        discovery_cache: DC,
        sender: MS,
        receiver: MR,
    ) -> Self {
        Self {
            routing_table,
            neighbor_table,
            discovery_cache,
            sender,
            receiver,
            _lt: PhantomData::default(),
        }
    }
}

impl<'a, RT, NT, DC, MS, MR, const ID_SIZE: usize, const BUCKET_SIZE: usize> Bootstrap<ID_SIZE>
    for BootstrapUseCase<'a, RT, NT, DC, MS, MR, ID_SIZE, BUCKET_SIZE>
where
    RT: RoutingTable<'a, ID_SIZE, BUCKET_SIZE>,
    NT: NeighborTable<ID_SIZE>,
    for<'b> &'b NT: IntoIterator<Item = (&'b NodeId<ID_SIZE>, &'b Interface)>,
    DC: DiscoveryTable<ID_SIZE>,
    MS: MessageSender<ID_SIZE>,
    MR: MessageReceiver<ID_SIZE>,
{
    fn start(&mut self, config: BootstrapConfig) -> Result<(), BootstrapError> {
        // Although the UseCase contains generating the NodeId
        // it's required to pass this before as the RoutingTable
        // and others require the root NodeId before Bootstrap is started.
        assert!(self.routing_table.root() != &NodeId::zero());

        // Send hello to all links
        if let Err(e) = self.sender.send(HelloMessage {
            source: self.routing_table.root().clone(),
            destination: NodeId::zero(),
        }) {
            log::error!("MessageSender failed: {}", e);
            return Err(BootstrapError::SendError);
        }

        // TODO: Wait for Responses or timeout

        // Wait for Neighbors to response
        // TODO: Move this to runtime
        std::thread::sleep(config.max_neighbor_response_duration);

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
        for (id, _) in &self.neighbor_table {
            if let Err(e) = self.sender.send(ReqRspMessage {
                // TODO: Register Nonce
                id: Nonce::random(),
                source: self.routing_table.root().clone(),
                destination: id.clone(),
                data: PNDiscRspData {
                    contacts: neighbors.clone(),
                },
            }) {
                log::error!("MessageSender failed: {}", e);
                return Err(BootstrapError::SendError);
            }
        }

        // TODO: Send QueryRouteReqs and wait for responses

        if let Err(e) = self.sender.send(ReqRspMessage {
            id: Nonce::random(),
            source: self.routing_table.root().clone(),
            destination: self.routing_table.root().clone(),
            data: FindNodeReqData {},
        }) {
            log::error!("MessageSender failed: {}", e);
            return Err(BootstrapError::SendError);
        }

        // TODO: Wait for FindNode Response to arrive

        Ok(())
    }
}
