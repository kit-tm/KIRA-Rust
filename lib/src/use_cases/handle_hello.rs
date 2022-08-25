use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};

use crate::context::UseCaseContext;
use crate::domain::{
    node_id, Age, Contact, InsertionStrategy, InsertionStrategyResult, Path, RoutingTable,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    HelloMessage, Nonce, PNDiscReqData, ProtocolMessage, ProtocolMessageSender, ReqRspMessage,
};
use crate::use_cases::{UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Clone)]
pub struct HandleHelloConfig {
    pub heuristic_calculation_bits: NonZeroUsize,
}

impl Default for HandleHelloConfig {
    fn default() -> Self {
        Self {
            heuristic_calculation_bits: NonZeroUsize::new(32).unwrap(),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Default)]
pub enum HandleHelloState {
    #[default]
    Idle,
    Error,
}

impl UseCaseState for HandleHelloState {
    fn is_finished(&self) -> bool {
        false
    }

    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum HandleHelloError {
    SendError,
}

impl Display for HandleHelloError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Failed to send message through message sender"),
        }
    }
}

impl Error for HandleHelloError {}

#[derive(Debug)]
pub struct HandleHelloUseCase<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    state: HandleHelloState,
    config: HandleHelloConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for HandleHelloUseCase<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(HandleHelloConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> HandleHelloUseCase<C, BUCKET_SIZE> {
    pub fn new(config: HandleHelloConfig) -> Self {
        if node_id::BIT_SIZE < config.heuristic_calculation_bits.get() {
            panic!("Number of bits to use for the heuristic in HandleHello is greater than BIT_SIZE of NodeId.")
        }
        Self {
            _c: PhantomData::default(),
            state: HandleHelloState::default(),
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for HandleHelloUseCase<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, BUCKET_SIZE>,
{
    type Context = C;
    type Error = HandleHelloError;
    type State = HandleHelloState;

    fn start(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        // Nothing to initialize here
        Ok(())
    }

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error> {
        if let UseCaseEvent::Message(
            ProtocolMessage::Hello(HelloMessage {
                source,
                source_state_seq_nr,
            }),
            in_port,
        ) = event
        {
            // Add or update Physical Neighbors
            if let Some(updated) = context.pn_table_mut().add(source.clone(), in_port.clone()) {
                log::debug!("Updated Port for PN: {}", updated);
            }

            // Try Inserting information into routing table
            let contact = Contact::new(
                Path::from(source.clone()),
                Age::from(0),
                source_state_seq_nr,
            );
            match context.routing_table_insertion_strategy().insert(
                contact.clone(),
                context.routing_table_mut().deref_mut(),
                context.pn_table().deref(),
            ) {
                InsertionStrategyResult::Inserted => {
                    log::debug!("Inserted new contact '{}'", source)
                }
                InsertionStrategyResult::Replaced(id) => {
                    log::debug!("Replaced contact '{}' with '{}'", id, source)
                }
                InsertionStrategyResult::Updated => {
                    log::debug!("Updated contact information '{}'", contact)
                }
                InsertionStrategyResult::Dropped => {
                    log::debug!("Dropped contact information '{}'", contact)
                }
            }

            // Answer with a PNDiscReq to ensure bidirectional connectivity
            let pn_contacts = context
                .pn_table()
                .into_iter()
                .filter_map(|(id, _)| context.routing_table().contact(id).cloned())
                .collect::<Vec<_>>();

            // Use deterministic heuristic to determine if we should respond to the Message with a
            // do not use full ID, otherwise large IDs will always "loose", use mod 2^{calculation_bits} comparison
            // small collision chance: but just in case, full nodeID will be a tie breaker

            // Unwrapping is safe here, as checked at construction
            let own_bits = context
                .root_id()
                .bits(0, self.config.heuristic_calculation_bits)
                .unwrap();
            let other_bits = source
                .bits(0, self.config.heuristic_calculation_bits)
                .unwrap();
            let delta = other_bits.wrapping_sub(own_bits);

            // Inverted (delta < 0x80000000) || ((delta == 0 || delta == 0x80000000) && context.root_id() < &source)
            if delta >= 0x80000000
                && ((delta != 0 && delta != 0x80000000) || context.root_id() >= &source)
            {
                return Ok(());
            }

            let message = ReqRspMessage {
                nonce: Nonce::random(),
                source: context.root_id().clone(),
                target: source.clone(),
                data: PNDiscReqData {
                    contacts: pn_contacts,
                },
                // Source route is ignored, as only physical neighbors get these
                source_route: SourceRoute::from(Path::from(source)),
            };
            if let Err(e) = context.message_sender_mut().send(message, in_port) {
                log::error!("Failed to send message: {}", e);
                self.state = HandleHelloState::Error;
                return Err(HandleHelloError::SendError);
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(all(test, feature = "bus"))]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::Arc;

    use crate::broadcaster::BusBroadcaster;
    use crate::context::SyncContext;
    use crate::domain::physical_neighbor_table::PNTable;
    use crate::domain::{
        FlatRoutingTable, InsertionStrategyResult, NodeId, StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{HelloMessage, InMemoryMessageHub, ProtocolMessage};
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::handle_hello::{HandleHelloConfig, HandleHelloUseCase};
    use crate::use_cases::{UseCase, UseCaseEvent};

    #[test]
    fn responds_with_pn_disc_req() {
        crate::tests::init();

        // Answer should be a PNDiscReq if
        let root_id = NodeId::with_lsb(2);
        let sender_id = NodeId::with_lsb(4);

        let routing_table =
            FlatRoutingTable::<20, 1>::new(root_id.clone()).expect("invalid grouping");

        let pn_table = PNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let message_hub = ArcSyncInMemoryMessageHub::new();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

        let mut use_case = HandleHelloUseCase::new(HandleHelloConfig {
            heuristic_calculation_bits: NonZeroUsize::new(34).unwrap(),
        });

        assert!(use_case.start(&context).is_ok());
        assert_eq!(
            use_case.handle_event(
                &context,
                UseCaseEvent::Message(
                    ProtocolMessage::Hello(HelloMessage {
                        source: sender_id.clone(),
                        source_state_seq_nr: StateSeqNr::from(0),
                    }),
                    InMemoryMessageHub::dummy_port(),
                ),
            ),
            Ok(())
        );

        let pn_disc_req = message_hub
            .messages()
            .iter()
            .find_map(|message| {
                if let ProtocolMessage::PNDiscReq(message) = &message.0 {
                    Some(message)
                } else {
                    None
                }
            })
            .cloned();
        assert!(pn_disc_req.is_some());
        let message = pn_disc_req.unwrap();
        assert_eq!(&message.source, &root_id);
        assert_eq!(&message.target, &sender_id);
    }

    #[test]
    fn doesnt_respond_with_pn_disc_req() {
        crate::tests::init();

        // Answer should be a PNDiscReq

        let root_id = NodeId::with_lsb(2);
        let sender_id = NodeId::with_lsb(1);

        let routing_table =
            FlatRoutingTable::<20, 1>::new(root_id.clone()).expect("invalid grouping");

        let pn_table = PNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let message_hub = ArcSyncInMemoryMessageHub::new();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

        let mut use_case = HandleHelloUseCase::default();

        assert!(use_case.start(&context).is_ok());
        assert_eq!(
            use_case.handle_event(
                &context,
                UseCaseEvent::Message(
                    ProtocolMessage::Hello(HelloMessage {
                        source: sender_id.clone(),
                        source_state_seq_nr: StateSeqNr::from(0),
                    }),
                    InMemoryMessageHub::dummy_port(),
                ),
            ),
            Ok(())
        );

        assert!(message_hub.messages().is_empty());
    }
}
