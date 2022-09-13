use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;

use crate::context::UseCaseContext;
use crate::domain::RoutingTable;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    ProtocolMessage, ProtocolMessageSender, QueryRouteType, RTableData, ReqRspMessage,
};
use crate::use_cases::{ReactiveUseCaseState, UseCase, UseCaseEvent};

#[derive(Debug)]
pub struct HandleVicinityDiscovery<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    state: ReactiveUseCaseState,
}

impl<C, const BUCKET_SIZE: usize> Default for HandleVicinityDiscovery<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            _c: PhantomData::default(),
            state: ReactiveUseCaseState::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> HandleVicinityDiscovery<C, BUCKET_SIZE> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for HandleVicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = HandleVDError;
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        // Nothing to initialize here
        Ok(())
    }

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error> {
        if let UseCaseEvent::Message(ProtocolMessage::QueryRouteReq(request), ..) = event {
            if request.destination() != context.root_id() {
                return Ok(());
            }

            let contacts = match request.data.query_type {
                QueryRouteType::PhysicalNeighbors => {
                    let pn_lock = context.pn_table();
                    let rt_lock = context.routing_table();

                    let result: Result<Vec<_>, HandleVDError> = pn_lock
                        .keys()
                        .map(|id| {
                            rt_lock
                                .contact(id)
                                .cloned()
                                .ok_or(HandleVDError::NeighborInconsistency)
                        })
                        .collect();
                    result?
                }
            };

            let message = ProtocolMessage::QueryRouteRsp(ReqRspMessage {
                nonce: request.nonce,
                source_state_seq_nr: *context.pn_table().state_seq_nr(),
                data: RTableData { contacts },
                source_route: SourceRoute::from_reversed(request.source_route),
            });

            log::trace!(target: "handle_vicinity_discovery", "Sending: {:?}", message);

            if let Err(e) = context.message_sender_mut().send(message) {
                log::error!(target: "handle_vicinity_discovery", "Failed to send: {}", e);
                return Err(HandleVDError::MessageSendFailed);
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

/// Error types for vicinity discovery.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum HandleVDError {
    /// Sending a ProtocolMessage failed.
    MessageSendFailed,
    /// A Contact contains an invalid neighbor.
    NeighborInconsistency,
}

impl Display for HandleVDError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MessageSendFailed => write!(
                f,
                "Sending a ProtocolMessage through a MessageSender failed"
            ),
            Self::NeighborInconsistency => write!(f, "No contact for physical neighbor present"),
        }
    }
}

impl Error for HandleVDError {}

#[cfg(test)]
mod tests {
    use crate::broadcaster::BusBroadcaster;
    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, InsertionStrategyResult, NodeId, PNTable, Path, RoutingTable, StateSeqNr,
        TestInsertionStrategy,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        InMemoryMessageHub, Nonce, ProtocolMessage, ProtocolMessageReceiver, QueryRouteReqData,
        QueryRouteType, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::handle_vicinity_discovery::HandleVicinityDiscovery;
    use crate::use_cases::{UseCase, UseCaseEvent};

    #[test]
    fn answers_with_pns() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let neighbors_port = InMemoryMessageHub::dummy_port();
        let neighbor_contacts = vec![
            Contact::new(Path::from([NodeId::with_msb(14)]), StateSeqNr::from(14)),
            Contact::new(Path::from([NodeId::with_msb(16)]), StateSeqNr::from(16)),
            Contact::new(Path::from([NodeId::with_msb(5)]), StateSeqNr::from(5)),
            Contact::new(Path::from([NodeId::with_msb(18)]), StateSeqNr::from(18)),
        ];

        let mut single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();

        for contact in &neighbor_contacts {
            pn_table.insert(contact.id().clone(), neighbors_port.clone());
            let insertion_result = single_bucket_rt.insert(contact.clone());
            assert!(
                insertion_result.is_ok(),
                "Insertion returned error: {:?}",
                insertion_result
            );
        }

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let mut message_hub = ArcSyncInMemoryMessageHub::new();
        let broadcaster = BusBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(
            root_id.clone(),
            single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

        let mut use_case = HandleVicinityDiscovery::default();
        let start_result = use_case.start(&context);
        assert!(
            start_result.is_ok(),
            "Start returned error: {:?}",
            start_result
        );

        let protocol_message = ProtocolMessage::QueryRouteReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(3),
            data: QueryRouteReqData {
                query_type: QueryRouteType::PhysicalNeighbors,
            },
            source_route: SourceRoute::from(Path::from([
                source_id.clone(),
                NodeId::with_msb(28),
                NodeId::with_msb(14),
                root_id.clone(),
            ])),
        });
        let event = UseCaseEvent::Message(protocol_message.clone(), neighbors_port);

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = message_hub.try_recv();
        assert!(
            sent_message.is_ok(),
            "Trying to receive returned error: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message sent by use case");
        let (sent_message, _) = sent_message.unwrap();

        assert_eq!(sent_message.nonce(), protocol_message.nonce());
        assert_eq!(
            sent_message.source_route(),
            Some(&SourceRoute::from(Path::from([
                root_id.clone(),
                NodeId::with_msb(14),
                NodeId::with_msb(28),
                source_id.clone(),
            ])))
        );
        let req = if let ProtocolMessage::QueryRouteRsp(inner_req) = sent_message.clone() {
            Some(inner_req)
        } else {
            None
        };
        assert!(
            req.is_some(),
            "Returned a different ProtocolMessage than QueryRouteRsp: {:?}",
            sent_message
        );
        let req = req.unwrap();
        // Check unordered equality
        assert_eq!(req.data.contacts.len(), neighbor_contacts.len());
        for neighbor in &neighbor_contacts {
            assert!(req.data.contacts.contains(&neighbor));
        }
    }

    #[test]
    fn ignores_messages_for_others() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let mut message_hub = ArcSyncInMemoryMessageHub::new();
        let broadcaster = BusBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(
            root_id.clone(),
            single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

        let mut use_case = HandleVicinityDiscovery::default();
        let start_result = use_case.start(&context);
        assert!(
            start_result.is_ok(),
            "Start returned error: {:?}",
            start_result
        );

        let protocol_message = ProtocolMessage::QueryRouteReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(3),
            data: QueryRouteReqData {
                query_type: QueryRouteType::PhysicalNeighbors,
            },
            source_route: SourceRoute::from(Path::from([
                source_id.clone(),
                NodeId::with_msb(28),
                NodeId::with_msb(14),
            ])),
        });
        let event =
            UseCaseEvent::Message(protocol_message.clone(), InMemoryMessageHub::dummy_port());

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = message_hub.try_recv();
        assert!(
            sent_message.is_ok(),
            "Trying to receive returned error: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(
            sent_message.is_none(),
            "No message should be sent bei use case"
        );
    }
}
