use std::marker::PhantomData;
use std::ops::Deref;
use std::{error::Error, fmt::Display, time::Duration};

use crate::context::Context;
use crate::domain::DiscoveryTable;
use crate::messaging::{FindNodeReqData, QueryRouteReqData};
use crate::runtime::Runtime;
use crate::usecases::{TimerId, UseCaseEvent};
use crate::{
    domain::{Contact, Interface, NeighborTable, NodeId, RoutingTable},
    messaging::{
        DiscRspData, HelloMessage, Message, MessageSender, Nonce, RTableReqType, ReqRspMessage,
    },
};

#[derive(Debug)]
pub enum BootstrapError {
    SendError,
    NoNeighbors,
}

impl Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Failed to send a message over the given MessageSender"),
            Self::NoNeighbors => {
                write!(f, "No Neighbors responded in time or the Node is isolated")
            }
        }
    }
}

impl Error for BootstrapError {}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum BootstrapState {
    Finished,
    Initialized,
    WaitingForNeighbors(TimerId),
    WaitingFor2HopVicinity(Vec<Nonce>, TimerId),
    WaitingForFindNodeResponse(Nonce, Option<TimerId>),
    Error,
}

#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    pub max_neighbor_response_duration: Duration,
    pub max_2_hop_response_duration: Duration,
    pub max_find_node_response_duration: Option<Duration>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            max_2_hop_response_duration: Duration::from_secs(1),
            max_neighbor_response_duration: Duration::from_secs(1),
            max_find_node_response_duration: Some(Duration::from_secs(1)),
        }
    }
}

/// The UseCase which represents the Bootstrap Process.
///
/// Performs NeighborDiscovery, 3-Hop-Vicinity Discovery and the initial join to the network.
#[derive(Debug)]
pub struct BootstrapUseCase<C, RT, NT, DT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    _rt: PhantomData<RT>,
    _nt: PhantomData<NT>,
    _dt: PhantomData<DT>,
    _ms: PhantomData<MS>,
    _ru: PhantomData<RU>,
    state: BootstrapState,
}

impl<C, RT, DT, NT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize> Default
    for BootstrapUseCase<C, RT, DT, NT, MS, RU, ID_SIZE, BUCKET_SIZE>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<C, RT, DT, NT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    BootstrapUseCase<C, RT, DT, NT, MS, RU, ID_SIZE, BUCKET_SIZE>
{
    pub fn new() -> Self {
        Self {
            _c: PhantomData::default(),
            _rt: PhantomData::default(),
            _nt: PhantomData::default(),
            _dt: PhantomData::default(),
            _ms: PhantomData::default(),
            _ru: PhantomData::default(),
            state: BootstrapState::Initialized,
        }
    }
}

impl<C, RT, DT, NT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    BootstrapUseCase<C, RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
where
    C: Context<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>,
    RT: RoutingTable<ID_SIZE, BUCKET_SIZE>,
    for<'b> &'b RT: IntoIterator<Item = &'b Contact<ID_SIZE>>,
    NT: NeighborTable<ID_SIZE>,
    for<'b> &'b NT: IntoIterator<Item = (&'b NodeId<ID_SIZE>, &'b Interface)>,
    MS: MessageSender<ID_SIZE>,
    DT: DiscoveryTable<ID_SIZE>,
    RU: Runtime,
{
    fn send_message<M: Into<Message<ID_SIZE>>>(
        &mut self,
        context: &C,
        message: M,
    ) -> Result<(), BootstrapError> {
        if let Err(e) = context.message_sender_mut().send(message) {
            log::error!("MessageSender failed: {}", e);
            self.state = BootstrapState::Error;
            return Err(BootstrapError::SendError);
        }

        Ok(())
    }

    /// Sends QueryRouteReq to discover the state of all Nodes in a 3 Hop vicinity.
    fn start_vicinity_discovery(
        &mut self,
        context: &C,
        config: &BootstrapConfig,
    ) -> Result<(), BootstrapError> {
        if context.neighbor_table().is_empty() {
            return Err(BootstrapError::NoNeighbors);
        }

        let two_hop_vicinity = context
            .routing_table()
            .deref()
            .into_iter()
            .filter_map(|contact: &Contact<ID_SIZE>| {
                if contact.path().len() == 1 {
                    // Neighbors => path.len() = 0, 1-Hop Neighbors => path.len() = 1
                    Some(contact.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        // Already discovered if neighbors are present but two hop vicinity has no entries
        // Although its not possible to discover more nodes then, perform join anyways
        if two_hop_vicinity.is_empty() {
            return self.start_join(context, config);
        }

        let mut nonces = Vec::with_capacity(two_hop_vicinity.len());
        for contact in two_hop_vicinity {
            let nonce = Nonce::random();
            nonces.push(nonce.clone());
            let message = ReqRspMessage {
                nonce,
                source: context.root_id().clone(),
                destination: contact.id().clone(),
                data: QueryRouteReqData {
                    req_type: RTableReqType::NeighborHood(1),
                },
            };
            self.send_message(context, message)?;
        }

        let timer_id = context
            .runtime()
            .register_timer(config.max_2_hop_response_duration);

        self.state = BootstrapState::WaitingFor2HopVicinity(nonces, timer_id);

        Ok(())
    }

    fn start_join(&mut self, context: &C, config: &BootstrapConfig) -> Result<(), BootstrapError> {
        let nonce = Nonce::random();
        let message = ReqRspMessage {
            nonce: nonce.clone(),
            source: context.root_id().clone(),
            destination: NodeId::zero(),
            data: FindNodeReqData {
                req_type: RTableReqType::ContactsOnly,
            },
        };

        self.send_message(context, message)?;

        let opt_timer_id = config
            .max_find_node_response_duration
            .map(|duration| context.runtime().register_timer(duration));

        self.state = BootstrapState::WaitingForFindNodeResponse(nonce, opt_timer_id);

        Ok(())
    }

    fn handle_query_route_rsp(
        &mut self,
        context: &C,
        config: &BootstrapConfig,
        message: ReqRspMessage<DiscRspData<ID_SIZE>, ID_SIZE>,
    ) -> Result<(), BootstrapError> {
        // Remove nonces if possible
        if let BootstrapState::WaitingFor2HopVicinity(nonces, _) = &mut self.state {
            if let Some(index) = nonces.iter().position(|stored| &message.nonce == stored) {
                nonces.swap_remove(index);
            }
            // All requested nodes responded -> start join early before timer goes off
            if nonces.is_empty() {
                self.start_join(context, config)?;
            }
        }

        Ok(())
    }
}

impl<C, RT, DT, NT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    BootstrapUseCase<C, RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
where
    C: Context<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>,
    RT: RoutingTable<ID_SIZE, BUCKET_SIZE>,
    for<'b> &'b RT: IntoIterator<Item = &'b Contact<ID_SIZE>>,
    NT: NeighborTable<ID_SIZE>,
    for<'b> &'b NT: IntoIterator<Item = (&'b NodeId<ID_SIZE>, &'b Interface)>,
    MS: MessageSender<ID_SIZE>,
    DT: DiscoveryTable<ID_SIZE>,
    RU: Runtime,
{
    /// Starts the Bootstrap Process by sending [HelloMessage]s to all neighbors and registering
    /// a timeout which will notify the [BoostrapUseCase] through [handle_event].
    pub fn start(&mut self, context: &C, config: &BootstrapConfig) -> Result<(), BootstrapError> {
        // Although the UseCase contains generating the NodeId
        // it's required to pass this before as the RoutingTable
        // and others require the root NodeId before Bootstrap is started.
        assert_ne!(context.root_id(), &NodeId::zero());

        // Send hello to all links
        let message = HelloMessage {
            source: context.root_id().clone(),
            destination: NodeId::zero(),
        };
        self.send_message(context, message)?;

        let timer_id = context
            .runtime()
            .register_timer(config.max_neighbor_response_duration);

        self.state = BootstrapState::WaitingForNeighbors(timer_id);

        Ok(())
    }

    pub fn handle_event(
        &mut self,
        context: &C,
        config: &BootstrapConfig,
        event: UseCaseEvent<ID_SIZE>,
    ) -> Result<(), BootstrapError> {
        match (self.state(), event) {
            // Timeout for neighbors was reached => Send
            (BootstrapState::WaitingForNeighbors(waiting_id), UseCaseEvent::Timer(received_id)) => {
                if waiting_id == &received_id {
                    self.start_vicinity_discovery(context, config)?;
                }
            }
            // Timeout received for 2 Hop Vicinity
            (
                BootstrapState::WaitingFor2HopVicinity(_, waiting_id),
                UseCaseEvent::Timer(timer_id),
            ) => {
                if waiting_id == &timer_id {
                    self.start_join(context, config)?;
                }
            }
            // Some Node in 2 Hop vicinity responded
            (
                BootstrapState::WaitingFor2HopVicinity(_, _),
                UseCaseEvent::Message(Message::QueryRouteRsp(message)),
            ) => self.handle_query_route_rsp(context, config, message)?,
            // Error returned for FindNodeReq
            (
                BootstrapState::WaitingForFindNodeResponse(nonce, _),
                UseCaseEvent::Message(Message::Error(message)),
            ) => {
                if nonce == &message.nonce {
                    log::error!("FindNodeReq returned an Error: {:?}", message);
                    self.state = BootstrapState::Error;
                }
            }
            // FindNodeRsp received for our request
            (
                BootstrapState::WaitingForFindNodeResponse(nonce, _),
                UseCaseEvent::Message(Message::FindNodeRsp(message)),
            ) => {
                if nonce == &message.nonce {
                    self.state = BootstrapState::Finished;
                }
            }
            // Timer for FindNodeRsp finished
            (
                BootstrapState::WaitingForFindNodeResponse(_, Some(waiting_id)),
                UseCaseEvent::Timer(timer_id),
            ) => {
                if waiting_id == &timer_id {
                    log::error!("FindNodeRsp took to long");
                    self.state = BootstrapState::Error;
                }
            }
            _ => {}
        };

        Ok(())
    }

    pub fn state(&self) -> &BootstrapState {
        &self.state
    }
}
