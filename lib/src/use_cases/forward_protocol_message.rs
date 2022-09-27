use std::fmt::Debug;
use std::marker::PhantomData;

use crate::context::UseCaseContext;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ErrorData, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::use_cases::{EventHandler, HandlingResult, MessageSentFailed, UseCaseEvent};

/// Forwards protocol messages if the source route is valid.
///
/// Not one of the use cases mentioned in the design paper but one of the shared functionality
/// cases which have to be performed and checked before passing to the original use cases.
///
/// # Errors
///
/// Sends an error back if next node in the source route is invalid.
#[derive(Debug, Default)]
pub struct ForwardProtocolMessages<C> {
    _pd: PhantomData<C>,
}

impl<C> ForwardProtocolMessages<C> {
    pub fn new() -> Self {
        Self {
            _pd: Default::default(),
        }
    }
}

impl<C> ForwardProtocolMessages<C>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
{
    fn handle_next_hop_not_neighbor(
        &self,
        context: &C,
        message: ProtocolMessage,
    ) -> Result<(), <Self as EventHandler>::Error> {
        let error_message = ReqRspMessage {
            nonce: message.nonce().unwrap().clone(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: ErrorData::SegmentFailure,
            source_route: SourceRoute::from_reversed(message.source_route().unwrap().clone()),
        };

        if let Err(e) = context.message_sender_mut().send(error_message) {
            log::error!("Failed to reply with error message: {}", e);
            return Err(MessageSentFailed);
        }

        Ok(())
    }
}

impl<C> EventHandler for ForwardProtocolMessages<C>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = MessageSentFailed;
    type Value = HandlingResult;

    fn handle(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        // Get source route if present
        let (mut message, _) = match event {
            UseCaseEvent::Message(message, source_port) => (message, source_port),
            _ => return Ok(HandlingResult::NotHandled),
        };
        let source_route = message.source_route().cloned();
        if source_route.is_none() {
            return Ok(HandlingResult::NotHandled);
        }
        let source_route = source_route.unwrap();

        // Current hop has to be us
        if source_route.current_hop() != context.root_id() {
            log::error!(
                target: "forward_protocol_message",
                "Current hop of message {} is not us [{:?}]",
                source_route.current_hop(),
                message
            );
            return Ok(HandlingResult::Handled);
        }

        let next_hop = source_route.next_hop();

        // Is directed to us -> nothing to forward
        if next_hop.is_none() {
            return Ok(HandlingResult::NotHandled);
        }
        let next_hop = next_hop.unwrap();

        // From here on the message is assumed to be for us

        // Next hop is not a physical neighbor -> Error -> Drop
        let neighbor_port = context.pn_table().get(next_hop).cloned();
        if neighbor_port.is_none() {
            self.handle_next_hop_not_neighbor(context, message)?;
            return Ok(HandlingResult::Handled);
        }

        // Advance source route and send on port
        // Checked route before
        if let Some(route) = message.source_route_mut() {
            route.advance();
        }
        log::trace!(target: "forward_protocol_message", "Forwarding message {:?}", message);
        if let Err(e) = context.message_sender_mut().send(message) {
            log::error!("Failed to forward message: {}", e);
            return Err(MessageSentFailed);
        }

        Ok(HandlingResult::Handled)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;
    use std::time::Duration;

    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, InsertionStrategyResult, NodeId, PNTable, Path, Port, RoutingTable, StateSeqNr,
        TestInsertionStrategy,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageReceiver, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::forward_protocol_message::ForwardProtocolMessages;
    use crate::use_cases::{EventHandler, UseCaseEvent};

    #[test]
    fn forward_to_us_doesnt_forward() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), Port::Named(String::from("test")));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = ForwardProtocolMessages::new();

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(2),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: NodeId::random(),
            },
            source_route: SourceRoute::from(Path::from([neighbor.id().clone(), root_id.clone()]))
                .advanced(),
        };

        let result = use_case.handle(
            &sync_context,
            UseCaseEvent::Message(message.into(), Port::new(String::from("test"))),
        );
        assert!(
            result.is_ok(),
            "Handling a valid message returned an error: {:?}",
            result
        );

        assert!(broadcast_receiver.try_recv().is_err());

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

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::with_lsb(2);
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), Port::Named(String::from("test")));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = ForwardProtocolMessages::new();

        let foreign_id = NodeId::with_lsb(3);
        let sent_request = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: NodeId::random(),
            },
            source_route: SourceRoute::from(Path::from([
                foreign_id,
                root_id.clone(),
                neighbor_id.clone(),
            ])),
        };

        let result = use_case.handle(
            &sync_context,
            UseCaseEvent::Message(sent_request.clone().into(), Port::new(String::from("test"))),
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
            assert_eq!(&req.nonce, &sent_request.nonce);
            assert_eq!(req.source(), sent_request.source());
            assert_eq!(req.destination(), sent_request.destination());
            let mut route = sent_request.source_route.clone();
            route.advance();
            assert_eq!(&req.source_route, &route);
            assert_eq!(&req.data, &sent_request.data);
        }
    }
}
