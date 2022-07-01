use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::context::UseCaseContext;
use crate::domain::{
    Age, Contact, InsertionStrategy, InsertionStrategyResult, NeighborTable, NodeId, Path, Port,
    RoutingTable,
};
use crate::messaging::{
    HelloMessage, Nonce, PNDiscReqData, ProtocolMessage, ProtocolMessageSender, RTableReqType,
    ReqRspMessage,
};
use crate::use_cases::{UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Eq, PartialEq)]
pub enum HandleHelloState {
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
}

impl<C, const BUCKET_SIZE: usize> Default for HandleHelloUseCase<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, const BUCKET_SIZE: usize> HandleHelloUseCase<C, BUCKET_SIZE> {
    pub fn new() -> Self {
        Self {
            _c: PhantomData::default(),
            state: HandleHelloState::Idle,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for HandleHelloUseCase<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::NeighborTable: NeighborTable,
    C::RoutingTable: RoutingTable<BUCKET_SIZE>,
    for<'a> &'a C::NeighborTable: IntoIterator<Item = (&'a NodeId, &'a Port)>,
    C::MessageSender: ProtocolMessageSender,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::NeighborTable, BUCKET_SIZE>,
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
                destination,
            }),
            in_port,
        ) = event
        {
            // Ignore messages not directed to us
            if &destination != context.root_id() {
                return Ok(());
            }

            // Add or update Physical Neighbors
            if let Some(updated) = context.neighbor_table_mut().add(source.clone(), in_port) {
                log::debug!("Updated Port for PN: {}", updated);
            }

            // Try Inserting information into routing table
            let contact = Contact::new(
                source.clone(),
                Age::from(0),
                Path::empty(),
                source_state_seq_nr,
            );
            match context.routing_table_insertion_strategy().insert(
                contact.clone(),
                context.routing_table_mut().deref_mut(),
                context.neighbor_table().deref(),
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
            let neighbor_contacts = context
                .neighbor_table()
                .into_iter()
                .filter_map(|(id, _)| context.routing_table().contact(id).cloned())
                .collect::<Vec<_>>();

            let message = ReqRspMessage {
                nonce: Nonce::random(),
                source: destination,
                destination: source,
                data: PNDiscReqData {
                    req_type: RTableReqType::NeighborHood(1),
                    contacts: neighbor_contacts,
                },
            };
            if let Err(e) = context.message_sender_mut().send(message) {
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
    use std::sync::Arc;

    use crate::broadcaster::BusBroadcaster;
    use crate::context::SyncContext;
    use crate::domain::neighbor_hash_table::NeighborHashTable;
    use crate::domain::{
        FlatRoutingTable, InsertionStrategyResult, NodeId, StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{HelloMessage, InMemoryMessageHub, ProtocolMessage, ReqRspMessage};
    use crate::runtime::DummyRuntime;
    use crate::use_cases::handle_hello::HandleHelloUseCase;
    use crate::use_cases::{UseCase, UseCaseEvent};

    #[test]
    fn responds_with_pn_disc_req() {
        // Answer should be a PNDiscReq

        let root_id = NodeId::random();
        let sender_id = NodeId::random();

        let routing_table =
            FlatRoutingTable::<20, 1>::new(root_id.clone()).expect("invalid grouping");

        let neighbor_table = NeighborHashTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let message_hub = ArcSyncInMemoryMessageHub::new();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let runtime = DummyRuntime::new(broadcaster);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            neighbor_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

        let mut use_case = HandleHelloUseCase::new();

        assert!(use_case.start(&context).is_ok());
        assert_eq!(
            use_case.handle_event(
                &context,
                UseCaseEvent::Message(
                    ProtocolMessage::Hello(HelloMessage {
                        source: sender_id.clone(),
                        source_state_seq_nr: StateSeqNr::from(0),
                        destination: root_id.clone(),
                    }),
                    InMemoryMessageHub::dummy_port(),
                ),
            ),
            Ok(())
        );

        assert!(message_hub.messages().iter().any(|message| {
            if let ProtocolMessage::PNDiscReq(ReqRspMessage {
                source,
                destination,
                ..
            }) = message
            {
                source == &root_id && destination == &sender_id
            } else {
                false
            }
        }));
    }
}
