use std::{
    error::Error,
    fmt::Display,
    time::{Duration, Instant},
};

use crate::{
    domain::{Contact, Interface, NeighborTable, NodeId, RoutingTable},
    messaging::{
        FindNodeReqData, FindNodeRspData, HelloMessage, Message, MessageReceiver, MessageSender,
        Nonce, PNDiscReqData, PNDiscRspData, RTableReqType, ReqRspMessage,
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

pub struct BootstrapUseCase<'a, RT, NT, MS, MR, const ID_SIZE: usize, const BUCKET_SIZE: usize> {
    root_id: NodeId<ID_SIZE>,
    routing_table: &'a mut RT,
    neighbor_table: &'a mut NT,
    sender: &'a mut MS,
    receiver: &'a mut MR,
}

impl<'a, RT, NT, MS, MR, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    BootstrapUseCase<'a, RT, NT, MS, MR, ID_SIZE, BUCKET_SIZE>
{
    pub fn new(
        root_id: NodeId<ID_SIZE>,
        routing_table: &'a mut RT,
        neighbor_table: &'a mut NT,
        sender: &'a mut MS,
        receiver: &'a mut MR,
    ) -> Self {
        Self {
            root_id,
            routing_table,
            neighbor_table,
            sender,
            receiver,
        }
    }
}

impl<'a, RT, NT, MS, MR, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    BootstrapUseCase<'a, RT, NT, MS, MR, ID_SIZE, BUCKET_SIZE>
where
    RT: RoutingTable<ID_SIZE, BUCKET_SIZE>,
    for<'b> &'b RT: IntoIterator<Item = &'b Contact<ID_SIZE>>,
    NT: NeighborTable<ID_SIZE>,
    for<'b> &'b NT: IntoIterator<Item = (&'b NodeId<ID_SIZE>, &'b Interface)>,
    MS: MessageSender<ID_SIZE>,
    MR: MessageReceiver<ID_SIZE>,
{
    fn handle_message(
        &mut self,
        (message, interface): (Message<ID_SIZE>, Interface),
    ) -> Result<(), BootstrapError> {
        match message {
            Message::PNDiscReq(message) => self.handle_pn_disc_req((message, interface)),
            _ => todo!(),
        }
    }

    fn handle_pn_disc_req(
        &mut self,
        (message, interface): (ReqRspMessage<PNDiscReqData<ID_SIZE>, ID_SIZE>, Interface),
    ) -> Result<(), BootstrapError> {
        // Ignore messages not for us
        if message.destination == self.root_id {
            return Ok(());
        }

        // Collect requested data
        let data = match message.data.req_type {
            RTableReqType::ContactsOnly => PNDiscRspData::ContactList(
                self.neighbor_table
                    .into_iter()
                    .map(|(id, _)| id.clone())
                    .collect(),
            ),
            RTableReqType::NeighborHood(neighborhood_size) => PNDiscRspData::RTable(
                self.routing_table
                    .into_iter()
                    .filter(|contact| contact.path().len() <= neighborhood_size)
                    .cloned()
                    .collect(),
            ),
        };

        // Send response
        if let Err(e) = self.sender.send(ReqRspMessage {
            nonce: message.nonce,
            source: self.root_id.clone(),
            destination: message.source.clone(),
            data,
        }) {
            log::error!("MessageSender failed: {}", e);
            return Err(BootstrapError::SendError);
        }

        // Add information to neighbor table
        self.neighbor_table.add(message.source, interface);

        Ok(())
    }
}

impl<'a, RT, NT, MS, MR, const ID_SIZE: usize, const BUCKET_SIZE: usize> Bootstrap<ID_SIZE>
    for BootstrapUseCase<'a, RT, NT, MS, MR, ID_SIZE, BUCKET_SIZE>
where
    RT: RoutingTable<ID_SIZE, BUCKET_SIZE>,
    for<'b> &'b RT: IntoIterator<Item = &'b Contact<ID_SIZE>>,
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

        // Measure time to abort after timeout was reached
        let start = Instant::now();
        let mut timeout_duration = config.max_neighbor_response_duration;
        while let Ok(Some(message)) = self.receiver.recv_timeout(Some(timeout_duration)) {
            // Only collect bidirectional connectivity requests
            self.handle_message(message)?;
            // Read all if some messages are received as batch
            // Possibly reduces calculation of elapsed time
            while let Ok(Some(message)) = self.receiver.try_recv() {
                self.handle_message(message)?;
            }

            // Finish if tiimeout was reached
            let elapsed = start.elapsed();
            if elapsed >= config.max_neighbor_response_duration {
                break;
            }
            // Otherwise calc new read timeout
            timeout_duration = config.max_neighbor_response_duration - elapsed;
        }

        // TODO: Send QueryRouteReqs and wait for responses

        let find_node_id_nonce = Nonce::random();
        if let Err(e) = self.sender.send(ReqRspMessage {
            nonce: find_node_id_nonce.clone(),
            source: self.root_id.clone(),
            destination: self.root_id.clone(),
            data: FindNodeReqData {},
        }) {
            log::error!("MessageSender failed: {}", e);
            return Err(BootstrapError::SendError);
        }

        // No timeout is used here as the FindNodeReq is assmued to
        // either return a FindNodeRsp or an Error.
        while let Some((message, _)) = self.receiver.recv() {
            match message {
                Message::FindNodeRsp(ReqRspMessage {
                    nonce,
                    data: FindNodeRspData { .. },
                    ..
                }) => {
                    if nonce == find_node_id_nonce {
                        break;
                    }
                }
                Message::Error(ReqRspMessage { nonce, .. }) => {
                    if nonce == find_node_id_nonce {
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
