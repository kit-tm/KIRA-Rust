use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;

use crate::context::UseCaseContext;
use crate::domain::{RoutingTable, DEFAULT_BUCKET_SIZE};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ProtocolMessage, ProtocolMessageSender, RTableData, ReqRspMessage};
use crate::use_cases::{ReactiveUseCaseState, UseCase, UseCaseEvent};

#[derive(Debug)]
pub struct HandlePNAdvertising<C, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    _pd: PhantomData<C>,
    state: ReactiveUseCaseState,
}

impl<C, const BUCKET_SIZE: usize> Default for HandlePNAdvertising<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, const BUCKET_SIZE: usize> HandlePNAdvertising<C, BUCKET_SIZE> {
    pub fn new() -> Self {
        Self {
            _pd: Default::default(),
            state: ReactiveUseCaseState::Idle,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for HandlePNAdvertising<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = HandlePNError;
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
        if let UseCaseEvent::Message(ProtocolMessage::PNDiscReq(req), _) = event {
            if req.destination() != context.root_id() {
                return Ok(());
            }

            // Note: Locks will be released at end of curly braces
            let (ssn, contacts) = {
                let rt_lock = context.routing_table();
                let pn_lock = context.pn_table();

                let neighbors = pn_lock.keys().collect::<Vec<_>>();
                let mut contacts = Vec::with_capacity(neighbors.len());
                for pn_id in neighbors {
                    let contact = rt_lock.contact(pn_id).cloned();
                    if contact.is_none() {
                        return Err(HandlePNError::NeighborInconsistency);
                    }
                    contacts.push(contact.unwrap());
                }
                (*pn_lock.state_seq_nr(), contacts)
            };

            let response = ProtocolMessage::PNDiscRsp(ReqRspMessage {
                nonce: req.nonce,
                source_state_seq_nr: ssn,
                data: RTableData { contacts },
                source_route: SourceRoute::from_reversed(req.source_route),
            });

            log::trace!(target: "handle_pn_advertising", "Sending: {:?}", response);

            if let Err(e) = context.message_sender_mut().send(response) {
                log::error!(target: "handle_pn_advertising", "Failed to send PNDiscRsp: {}", e);
                return Err(HandlePNError::SendError);
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum HandlePNError {
    SendError,
    NeighborInconsistency,
}

impl Display for HandlePNError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Failed to send PNDiscReq"),
            Self::NeighborInconsistency => {
                write!(
                    f,
                    "Physical neighbors should be in PNTable and RoutingTable"
                )
            }
        }
    }
}

impl Error for HandlePNError {}

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
        InMemoryMessageHub, Nonce, ProtocolMessage, ProtocolMessageReceiver, RTableData,
        ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::handle_pn_advertising::HandlePNAdvertising;
    use crate::use_cases::{UseCase, UseCaseEvent};

    #[test]
    fn returns_pn_contacts() {
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

        let mut use_case = HandlePNAdvertising::default();
        let start_result = use_case.start(&context);
        assert!(
            start_result.is_ok(),
            "Start returned error: {:?}",
            start_result
        );

        let protocol_message = ProtocolMessage::PNDiscReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(3),
            data: RTableData { contacts: vec![] },
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
        let req = if let ProtocolMessage::PNDiscRsp(inner_req) = sent_message.clone() {
            Some(inner_req)
        } else {
            None
        };
        assert!(
            req.is_some(),
            "Returned a different ProtocolMessage than PNDiscRsp: {:?}",
            sent_message
        );
        let req = req.unwrap();
        // Check unordered equality
        assert_eq!(req.data.contacts.len(), neighbor_contacts.len());
        for neighbor in &neighbor_contacts {
            assert!(req.data.contacts.contains(&neighbor));
        }
    }
}
