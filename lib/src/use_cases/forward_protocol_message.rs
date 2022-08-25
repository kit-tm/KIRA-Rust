use std::fmt::Debug;
use std::marker::PhantomData;

use crate::context::UseCaseContext;
use crate::domain::Port;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ErrorData, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::use_cases::{MessageSentFailed, ReactiveUseCaseState, UseCase, UseCaseEvent};

#[derive(Debug, Default)]
pub struct ForwardPMUseCase<C> {
    _pd: PhantomData<C>,
    state: ReactiveUseCaseState,
}

impl<C> ForwardPMUseCase<C> {
    pub fn new() -> Self {
        Self {
            _pd: Default::default(),
            state: ReactiveUseCaseState::Idle,
        }
    }
}

impl<C> ForwardPMUseCase<C>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
{
    fn handle_next_hop_not_neighbor(
        &self,
        context: &C,
        message: ProtocolMessage,
        port: Port,
    ) -> Result<(), <Self as UseCase>::Error> {
        let error_message = ReqRspMessage {
            nonce: message.nonce().unwrap().clone(),
            source: context.root_id().clone(),
            target: message.source().unwrap().clone(),
            data: ErrorData::SegmentFailure,
            source_route: SourceRoute::from_reversed(message.source_route().unwrap().clone()),
        };

        if let Err(e) = context.message_sender_mut().send(error_message, port) {
            log::error!("Failed to reply with error message: {}", e);
            return Err(MessageSentFailed);
        }

        Ok(())
    }
}

impl<C> UseCase for ForwardPMUseCase<C>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = MessageSentFailed;
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        // We only react to incoming ProtocolMessages
        Ok(())
    }

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error> {
        // Get source route if present
        let (mut message, source_port) = match event {
            UseCaseEvent::Message(message, source_port) => (message, source_port),
            _ => return Ok(()),
        };
        let source_route = message.source_route_mut();
        if source_route.is_none() {
            log::error!("Received message with no source route: {:?}", message);
            return Ok(());
        }
        let source_route = source_route.unwrap();

        // Current hop has to be us
        if source_route.current_hop() != context.root_id() {
            log::error!(
                "Current hop of message is not us [{}]",
                source_route.current_hop()
            );
            return Ok(());
        }

        let next_hop = source_route.next_hop();

        // Is directed to us -> nothing to forward
        if next_hop.is_none() {
            return Ok(());
        }
        let next_hop = next_hop.unwrap();

        // Next hop is not a physical neighbor -> Error -> Drop
        let neighbor_port = context.pn_table().get(next_hop).cloned();
        if neighbor_port.is_none() {
            self.handle_next_hop_not_neighbor(context, message, source_port)?;
            return Ok(());
        }
        let neighbor_port = neighbor_port.unwrap();

        // Advance source route and send on port
        source_route.advance();
        if let Err(e) = context.message_sender_mut().send(message, neighbor_port) {
            log::error!("Failed to forward message: {}", e);
            return Err(MessageSentFailed);
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(all(test, feature = "bus"))]
mod tests {
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::broadcaster::{Broadcaster, BusBroadcaster};
    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Age, Contact, InsertionStrategyResult, NodeId, PNTable, Path, Port, RoutingTable,
        StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageReceiver, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::forward_protocol_message::ForwardPMUseCase;
    use crate::use_cases::{UseCase, UseCaseEvent};

    #[test]
    fn forward_to_us_doesnt_forward() {
        crate::tests::init();

        let root_id = NodeId::random();

        let broadcaster = Arc::new(BusBroadcaster::new(1));
        let mut subscriber = broadcaster.subscribe();

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let neighbor = Contact::new(
            Path::from(neighbor_id.clone()),
            Age::from(0),
            StateSeqNr::from(0),
        );

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
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

        let mut use_case = ForwardPMUseCase::new();

        assert!(use_case.start(&sync_context).is_ok());

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source: neighbor.id().clone(),
            target: NodeId::random(),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
            },
            source_route: SourceRoute::from(Path::from(root_id.clone())),
        };

        let result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(message.into(), Port::new(String::from("test"))),
        );
        assert!(
            result.is_ok(),
            "Handling a valid message returned an error: {:?}",
            result
        );

        assert!(subscriber.try_recv().is_err());

        assert!(
            hub.messages().is_empty(),
            "No messages should ne emitted, but these were found: {:?}",
            hub.messages()
        );
    }

    #[test]
    fn forwarding_works() {
        crate::tests::init();

        let root_id = NodeId::with_lsb(1);

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::with_lsb(2);
        let neighbor = Contact::new(
            Path::from(neighbor_id.clone()),
            Age::from(0),
            StateSeqNr::from(0),
        );

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
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

        let mut use_case = ForwardPMUseCase::new();

        assert!(use_case.start(&sync_context).is_ok());

        let foreign_id = NodeId::with_lsb(3);
        let request = ReqRspMessage {
            nonce: Nonce::random(),
            source: foreign_id,
            target: NodeId::random(),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
            },
            source_route: SourceRoute::from(Path::from([root_id.clone(), neighbor_id.clone()])),
        };

        let result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(request.clone().into(), Port::new(String::from("test"))),
        );
        assert!(
            result.is_ok(),
            "Handling a valid message returned an error: {:?}",
            result
        );

        let sent_message = hub.recv_timeout(Some(Duration::from_secs(1)));
        assert!(sent_message.is_ok(), "Timed out getting forwarded message");
        let message = sent_message.unwrap();
        assert!(message.is_some(), "Received no message from hub");
        let (message, _) = message.unwrap();
        if let ProtocolMessage::FindNodeReq(req) = message {
            assert_eq!(&req.nonce, &request.nonce);
            assert_eq!(&req.source, &request.source);
            assert_eq!(&req.target, &request.target);
            let mut route = request.source_route.clone();
            route.advance();
            assert_eq!(&req.source_route, &route);
            assert_eq!(&req.data, &request.data);
        }
    }
}
