use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;

use crate::context::UseCaseContext;
use crate::domain::ContactState;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    Nonce, ProtocolMessageSender, QueryRouteReqData, QueryRouteType, ReqRspMessage,
};
use crate::use_cases::{
    ContactEvent, MessageSentFailed, ReactiveUseCaseState, UseCase, UseCaseEvent, UseCaseState,
};

/// Radius of the neighborhood considered as vicinity.
///
/// A radius of **3** means:
///
/// - Contacts with a distance **<= 3** hops (*path length <= 4*) are in the vicinity.
/// - Contacts with a distance **< 3** hops (*path length < 4*) receive QueryRouteReqs.
///     Except the physical neighbors with a distance of 0 hops (*path length == 1*), which
///     are handled by the [HandleHelloUseCase] (*Hello* and *PNDiscReq/-Rsp*).
pub const VICINITY_RADIUS: usize = 3;

/// Error types for vicinity discovery.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum VDError {
    /// Sending a ProtocolMessage failed.
    MessageSendFailed,
    /// A Contact contains an invalid neighbor.
    NeighborInconsistency,
}

impl Display for VDError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MessageSendFailed => write!(
                f,
                "Sending a ProtocolMessage through a MessageSender failed"
            ),
            Self::NeighborInconsistency => write!(f, "Contact contains invalid neighbor"),
        }
    }
}

impl Error for VDError {}

/// The vicinity discovery (VD) use case.
///
/// Handles the discovery of the physical neighborhood (*vicinity*) beyond the direct
/// physical neighbors.
///
/// If a new [Contact] was added to the [RoutingTable] or an existing one was updated and
/// has a physical distance in the range of [1, [VICINITY_RADIUS]] hops (*path length is in
/// [2, [VICINITY_RADIUS] + 1]) a QueryRouteReq is sent to them to get their physical neighbors.
///
/// Physical Neighbors are already handled by the [HandleHelloUseCase] which is why
/// the range starts at 1 hop.
#[derive(Debug)]
pub struct VDUseCase<C> {
    _c: PhantomData<C>,
    state: ReactiveUseCaseState,
}

impl<C> Default for VDUseCase<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> VDUseCase<C> {
    /// Create a new vicinity discovery use case in [VDState::Idle].
    pub fn new() -> Self {
        Self {
            _c: PhantomData::default(),
            state: ReactiveUseCaseState::default(),
        }
    }
}

impl<C> UseCase for VDUseCase<C>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = VDError;
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        // Nothing to do here as we only react to Routing Table Updates
        Ok(())
    }

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error> {
        if let UseCaseEvent::Contact(ContactEvent::New(contact))
        | UseCaseEvent::Contact(ContactEvent::Updated(contact)) = event
        {
            // Physical Neighbors and Nodes outside of the Vicinity are not included
            if contact.is_pn() || contact.path().size() > VICINITY_RADIUS {
                return Ok(());
            }

            // Only Valid Contacts are considered
            if contact.state() != &ContactState::Valid {
                return Ok(());
            }

            // Convert contacts path to source route
            let route = SourceRoute::from(contact.path().clone());

            // Get port of route
            let neighbor_port = context.pn_table().get(contact.path().first()).cloned();
            if neighbor_port.is_none() {
                log::error!(
                    "Temporary inconsistency: Valid contacts path starts with invalid neighbor {}",
                    contact.path().first()
                );
                self.state = ReactiveUseCaseState::Error;
                return Err(VDError::NeighborInconsistency);
            }
            let neighbor_port = neighbor_port.unwrap();

            // Request only physical Neighborhood of that Node
            let request = ReqRspMessage {
                nonce: Nonce::random(),
                source: context.root_id().clone(),
                target: contact.id().clone(),
                data: QueryRouteReqData {
                    query_type: QueryRouteType::PhysicalNeighbors,
                },
                source_route: route,
            };

            if let Err(e) = context.message_sender_mut().send(request, neighbor_port) {
                log::error!("Failed to send QueryRouteReq: {:?}", e);
                return Err(VDError::MessageSendFailed);
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
    use std::time::Duration;

    use crate::broadcaster::BusBroadcaster;
    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Age, Contact, InsertionStrategyResult, NodeId, PNTable, Path, Port, RoutingTable,
        StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        ProtocolMessage, ProtocolMessageReceiver, QueryRouteReqData, QueryRouteType, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::vicinity_discovery::{VDUseCase, VICINITY_RADIUS};
    use crate::use_cases::{ContactEvent, UseCase, UseCaseEvent};

    #[test]
    fn no_request_for_pns() {
        crate::tests::init();

        let root_id = NodeId::random();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        let sync_context = SyncContext::new(
            root_id.clone(),
            SingleBucketRT::<1>::new(root_id),
            PNTable::new(),
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = VDUseCase::new();

        assert_eq!(use_case.start(&sync_context), Ok(()));

        let event = UseCaseEvent::Contact(ContactEvent::New(Contact::new(
            Path::from(NodeId::random()),
            Age::from(0),
            StateSeqNr::from(0),
        )));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        assert!(hub.messages().is_empty());
    }

    #[test]
    fn no_request_for_outside_vicinity() {
        crate::tests::init();

        let root_id = NodeId::random();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        let sync_context = SyncContext::new(
            root_id.clone(),
            SingleBucketRT::<1>::new(root_id),
            PNTable::new(),
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = VDUseCase::new();

        assert_eq!(use_case.start(&sync_context), Ok(()));

        // Build path bigger than vicinity radius
        let mut path = Path::from(NodeId::random());
        for _ in 0..VICINITY_RADIUS {
            path.push(NodeId::random());
        }

        let event = UseCaseEvent::Contact(ContactEvent::New(Contact::new(
            path,
            Age::from(0),
            StateSeqNr::from(0),
        )));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        assert!(hub.messages().is_empty());
    }

    #[test]
    fn request_for_inside_vicinity() {
        crate::tests::init();

        let root_id = NodeId::random();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table
            .insert(Contact::new(
                Path::from(neighbor_id.clone()),
                Age::from(0),
                StateSeqNr::from(0),
            ))
            .is_ok());
        let mut pn_table = PNTable::new();
        pn_table.add(neighbor_id.clone(), Port::Named(String::from("test")));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = VDUseCase::new();

        assert_eq!(use_case.start(&sync_context), Ok(()));

        // Build path bigger than vicinity radius
        let contact_id = NodeId::random();
        let path = Path::from(vec![neighbor_id, contact_id.clone()]);

        let event = UseCaseEvent::Contact(ContactEvent::New(Contact::new(
            path,
            Age::from(0),
            StateSeqNr::from(0),
        )));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        let (received, _) = hub
            .recv_timeout(Some(Duration::from_secs(1)))
            .expect("receiving should work")
            .expect("should return an actual message");

        if let ProtocolMessage::QueryRouteReq(ReqRspMessage {
            source,
            target,
            data:
                QueryRouteReqData {
                    query_type: QueryRouteType::PhysicalNeighbors,
                },
            ..
        }) = received
        {
            assert_eq!(source, root_id);
            assert_eq!(target, contact_id);
        }
    }
}
