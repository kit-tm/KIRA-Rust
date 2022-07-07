use std::marker::PhantomData;
use std::ops::Deref;
use std::{error::Error, fmt::Display, time::Duration};

use crate::context::UseCaseContext;
use crate::messaging::messages::{FindNodeReqData, QueryRouteReqData};
use crate::messaging::sender::ProtocolMessageSender;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};
use crate::{
    domain::{Contact, NodeId, RoutingTable},
    messaging::messages::{HelloMessage, Nonce, ProtocolMessage, RTableData, ReqRspMessage},
};

#[derive(Debug)]
pub enum BootstrapError {
    SendError,
    NoPhysicalNeighbors,
}

impl Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Failed to send a message over the given MessageSender"),
            Self::NoPhysicalNeighbors => {
                write!(
                    f,
                    "No Physical Neighbors responded in time or the Node is isolated"
                )
            }
        }
    }
}

impl Error for BootstrapError {}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum BootstrapState {
    Finished,
    Initialized,
    WaitingForPhysicalNeighbors(TimerId),
    WaitingFor2HopVicinity(Vec<Nonce>, TimerId),
    WaitingForFindNodeResponse(Nonce, Option<TimerId>),
    Error,
}

impl UseCaseState for BootstrapState {
    fn is_finished(&self) -> bool {
        self == &BootstrapState::Finished
    }

    fn is_error(&self) -> bool {
        self == &BootstrapState::Error
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BootstrapConfig {
    pub max_pn_response_duration: Duration,
    pub max_2_hop_response_duration: Duration,
    pub max_find_node_response_duration: Option<Duration>,
    pub initial_physical_neighborhood_size: u64,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            max_2_hop_response_duration: Duration::from_secs(1),
            max_pn_response_duration: Duration::from_secs(1),
            max_find_node_response_duration: Some(Duration::from_secs(1)),
            initial_physical_neighborhood_size: 20,
        }
    }
}

/// The UseCase which represents the Bootstrap Process.
///
/// Performs physical neighbor discovery, 3-Hop-Vicinity Discovery and the initial join to the network.
#[derive(Debug)]
pub struct BootstrapUseCase<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    state: BootstrapState,
    config: BootstrapConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for BootstrapUseCase<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(BootstrapConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> BootstrapUseCase<C, BUCKET_SIZE> {
    pub fn new(config: BootstrapConfig) -> Self {
        Self {
            _c: PhantomData::default(),
            state: BootstrapState::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> BootstrapUseCase<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::RoutingTable: RoutingTable<BUCKET_SIZE>,
    for<'b> &'b C::RoutingTable: IntoIterator<Item = &'b Contact>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
{
    fn send_message<M: Into<ProtocolMessage>>(
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
        if context.pn_table().is_empty() {
            return Err(BootstrapError::NoPhysicalNeighbors);
        }

        let two_hop_vicinity = context
            .routing_table()
            .deref()
            .into_iter() // TODO: Provide function for that in Routing Table?
            .filter_map(|contact: &Contact| {
                if contact.path().len() == 1 {
                    // Physical Neighbors => path.len() = 0, 1-Hop Neighbors => path.len() = 1
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
                data: QueryRouteReqData,
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
            data: FindNodeReqData { exact: false },
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
        message: ReqRspMessage<RTableData>,
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

impl<C, const BUCKET_SIZE: usize> UseCase for BootstrapUseCase<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::RoutingTable: RoutingTable<BUCKET_SIZE>,
    for<'b> &'b C::RoutingTable: IntoIterator<Item = &'b Contact>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
{
    type Context = C;
    type Error = BootstrapError;
    type State = BootstrapState;

    /// Starts the Bootstrap Process by sending [HelloMessage]s to all physical neighbors and
    /// registering a timeout which will notify the [BoostrapUseCase] through [handle_event].
    fn start(&mut self, context: &C) -> Result<(), BootstrapError> {
        // Although the UseCase contains generating the NodeId
        // it's required to pass this before as the RoutingTable
        // and others require the root NodeId before Bootstrap is started.
        assert_ne!(context.root_id(), &NodeId::zero());

        // Send hello to all links
        let message = HelloMessage {
            source: context.root_id().clone(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            destination: NodeId::zero(),
        };
        self.send_message(context, message)?;

        let timer_id = context
            .runtime()
            .register_timer(self.config.max_pn_response_duration);

        self.state = BootstrapState::WaitingForPhysicalNeighbors(timer_id);

        Ok(())
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), BootstrapError> {
        let config = self.config;
        match (self.state(), event) {
            // Timeout for physical neighbors was reached => Send
            (
                BootstrapState::WaitingForPhysicalNeighbors(waiting_id),
                UseCaseEvent::Timer(received_id),
            ) => {
                if waiting_id == &received_id {
                    self.start_vicinity_discovery(context, &config)?;
                }
            }
            // Timeout received for 2 Hop Vicinity
            (
                BootstrapState::WaitingFor2HopVicinity(_, waiting_id),
                UseCaseEvent::Timer(timer_id),
            ) => {
                if waiting_id == &timer_id {
                    self.start_join(context, &config)?;
                }
            }
            // Some Node in 2 Hop vicinity responded
            (
                BootstrapState::WaitingFor2HopVicinity(_, _),
                UseCaseEvent::Message(ProtocolMessage::QueryRouteRsp(message), ..),
            ) => self.handle_query_route_rsp(context, &config, message)?,
            // Error returned for FindNodeReq
            (
                BootstrapState::WaitingForFindNodeResponse(nonce, _),
                UseCaseEvent::Message(ProtocolMessage::Error(message), ..),
            ) => {
                if nonce == &message.nonce {
                    log::error!("FindNodeReq returned an Error: {:?}", message);
                    self.state = BootstrapState::Error;
                }
            }
            // FindNodeRsp received for our request
            (
                BootstrapState::WaitingForFindNodeResponse(nonce, _),
                UseCaseEvent::Message(ProtocolMessage::FindNodeRsp(message), ..),
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

    fn state(&self) -> &BootstrapState {
        &self.state
    }
}
