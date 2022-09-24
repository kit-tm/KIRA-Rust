use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::context::UseCaseContext;
use crate::domain::{Contact, InsertionStrategy, Path, Port, RoutingTable};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ProtocolMessage, RTableData, ReqRspMessage};
use crate::use_cases::{EventHandler, MessageSentFailed, UseCaseEvent};

/// Extracts different kinds of information out of incoming [ProtocolMessage]s.
///
/// Extracts these different kinds of information:
///
/// - The [Contact] information of the messages source will be added or updated.
/// - The neighbors [Contact] information as well as
///
/// As some [UseCase]s rely on the information already being extracted this UseCase has to handle
/// any [ProtocolMessage] before all other [UseCase]s.
#[derive(Debug)]
pub struct MessageInfoExtraction<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
}

impl<C, const BUCKET_SIZE: usize> Default for MessageInfoExtraction<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            _pd: PhantomData::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> MessageInfoExtraction<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, BUCKET_SIZE>,
{
    fn extract_path_to_source(&self, message: &ProtocolMessage) -> Path {
        let route = message.source_route().map(SourceRoute::traveled_path);
        let mut path = match route {
            None => Path::from(message.source().clone()),
            Some(path) => path,
        };

        path.reverse();

        path
    }

    /// Extracts source information, inserts it into the [PNTable] and [RoutingTable] and returns
    /// the extracted [Contact] information.
    fn extract_source_information(
        &self,
        context: &C,
        message: &ProtocolMessage,
        port: Port,
    ) -> Contact {
        let path = self.extract_path_to_source(message);
        let contact = Contact::new(path.clone(), *message.source_state_seq_nr());

        {
            let mut lock = context.pn_table_mut();
            let neighbor_id = path.first();
            if !lock.contains(neighbor_id) {
                if let Some(replaced) = lock.insert(neighbor_id.clone(), port.clone()) {
                    // Not allowed to happen as lock is held
                    log::warn!(
                        target: "pn_table",
                        "Overwritten port mapping for '{}' from '{}' to '{}' but checked before",
                        neighbor_id,
                        port,
                        replaced
                    );
                } else {
                    log::debug!(target: "pn_table", "Inserted neighbor '{}' at port '{}'", neighbor_id, port);
                }
            }
        }

        self.insert_contact(context, contact.clone());

        contact
    }

    fn insert_contact(&self, context: &C, contact: Contact) {
        context.routing_table_insertion_strategy().insert(
            contact,
            context.routing_table_mut().deref_mut(),
            context.pn_table().deref(),
        );
    }

    fn extract_rtable_reqrsp(
        &self,
        context: &C,
        request: ReqRspMessage<RTableData>,
        path_to_source: Path,
    ) {
        for mut contact in request.data.contacts {
            let mut path = path_to_source.clone();
            path.extend(contact.path().clone());
            *contact.path_mut() = path;

            self.insert_contact(context, contact);
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for MessageInfoExtraction<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, BUCKET_SIZE>,
{
    type Context = C;
    type Error = MessageSentFailed;
    type Value = ();

    fn handle(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let UseCaseEvent::Message(message, port) = event {
            if let ProtocolMessage::Hello(_) = message {
                return Ok(());
            }

            let source_contact = self.extract_source_information(context, &message, port);

            match message {
                ProtocolMessage::PNDiscReq(msg)
                | ProtocolMessage::PNDiscRsp(msg)
                | ProtocolMessage::QueryRouteRsp(msg)
                | ProtocolMessage::FindNodeRsp(msg) => {
                    self.extract_rtable_reqrsp(context, msg, source_contact.path().clone())
                }
                // These are already covered by source info extraction
                // Explicitly listing to yield compile time errors as soon as chnages happen to ProtocolMessage enum
                ProtocolMessage::Hello(_)
                | ProtocolMessage::QueryRouteReq(_)
                | ProtocolMessage::FindNodeReq(_)
                | ProtocolMessage::Error(_) => {}
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;
    use std::ops::Deref;

    use crate::broadcaster::BusBroadcaster;
    use crate::context::{SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, ContactState, InsertionStrategyResult, NodeId, PNTable, Path, Port, RoutingTable,
        StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::ProtocolMessage::{
        FindNodeReq, FindNodeRsp, PNDiscReq, PNDiscRsp, QueryRouteRsp,
    };
    use crate::messaging::{FindNodeReqData, Nonce, RTableData, ReqRspMessage};
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::message_info_extraction::MessageInfoExtraction;
    use crate::use_cases::{EventHandler, UseCaseEvent};

    #[test]
    fn extract_source_from_pndiscreq() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let contact_id = NodeId::with_msb(2);
        let port = Port::new(String::from("test"));
        let event = UseCaseEvent::Message(
            PNDiscReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(1),
                data: RTableData { contacts: vec![] },
                source_route: SourceRoute::from(Path::from([contact_id.clone(), root_id])),
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        assert_eq!(context.pn_table().get(&contact_id), Some(&port));
    }

    #[test]
    fn add_source_from_pndiscreq_to_contacts() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let neighbor_id = NodeId::with_msb(2);
        let port = Port::new(String::from("test"));
        let event = UseCaseEvent::Message(
            PNDiscReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(0),
                data: RTableData { contacts: vec![] },
                source_route: SourceRoute::from(Path::from([neighbor_id.clone(), root_id])),
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        assert_eq!(context.pn_table().get(&neighbor_id), Some(&port));

        let rt = context.routing_table();
        let saved_contact = rt.contact(&neighbor_id);
        assert!(saved_contact.is_some(), "No contact found");
        let saved_contact = saved_contact.unwrap();
        assert_eq!(saved_contact.path(), &Path::from(neighbor_id));
        assert_eq!(saved_contact.state_seq_nr(), &StateSeqNr::from(0));
        assert_eq!(saved_contact.state(), &ContactState::Valid);
    }

    #[test]
    fn add_source_to_routing_table() {
        crate::tests::init();

        let port = Port::new(String::from("test"));

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(42);

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        // NOTE: First element in contacts path has to be a neighbor
        pn_table.insert(neighbor_id.clone(), port.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            NodeId::with_msb(13),
            NodeId::with_msb(14),
            NodeId::with_msb(15),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        // One time advanced so the current_hop is the root_id
        .advanced()
        .advanced()
        .advanced()
        .advanced();
        let event = UseCaseEvent::Message(
            FindNodeReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: FindNodeReqData {
                    exact: false,
                    neighborhood: NonZeroU64::new(3).unwrap(),
                    target: NodeId::with_msb(15),
                },
                source_route,
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        let saved_contact = rt.contact(&source_id);
        assert!(saved_contact.is_some(), "No contact found");
        let saved_contact = saved_contact.unwrap();
        assert_eq!(
            saved_contact.path(),
            &Path::from([
                neighbor_id,
                NodeId::with_msb(15),
                NodeId::with_msb(14),
                NodeId::with_msb(13),
                source_id
            ])
        );
        assert_eq!(saved_contact.state_seq_nr(), &StateSeqNr::from(5));
        assert_eq!(saved_contact.state(), &ContactState::Valid);
    }

    #[test]
    fn add_new_neighbor_to_pns() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(3);

        let port = Port::new(String::from("test"));

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
            NodeId::with_msb(13),
            NodeId::with_msb(14),
            NodeId::with_msb(15),
        ]))
        // One time advanced so the current_hop is the root_id
        .advanced();
        let event = UseCaseEvent::Message(
            FindNodeReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: FindNodeReqData {
                    exact: false,
                    neighborhood: NonZeroU64::new(3).unwrap(),
                    target: NodeId::with_msb(15),
                },
                source_route,
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        assert_eq!(
            context.pn_table().get(&neighbor_id),
            Some(&port),
            "Neighbor not inserted in PNTable: {:?}",
            context.pn_table().deref()
        );
    }

    #[test]
    fn extract_infos_from_pn_disc_req() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let neighbor_id = NodeId::with_msb(2);

        let port = Port::new(String::from("test"));

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), port.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let neighbors_neighbors = vec![
            Contact::new(Path::from([NodeId::with_msb(15)]), StateSeqNr::from(15)),
            Contact::new(Path::from([NodeId::with_msb(30)]), StateSeqNr::from(30)),
            Contact::new(Path::from([NodeId::with_msb(42)]), StateSeqNr::from(42)),
        ];

        let source_route = SourceRoute::from(Path::from([neighbor_id.clone(), root_id.clone()]));
        let event = UseCaseEvent::Message(
            PNDiscReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                source_route,
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            assert_eq!(
                saved_contact.path(),
                &Path::from([neighbor_id.clone(), saved_contact.id().clone()])
            );
        }
    }

    #[test]
    fn extract_infos_from_pn_disc_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let neighbor_id = NodeId::with_msb(2);

        let port = Port::new(String::from("test"));

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), port.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let neighbors_neighbors = vec![
            Contact::new(Path::from([NodeId::with_msb(15)]), StateSeqNr::from(15)),
            Contact::new(Path::from([NodeId::with_msb(30)]), StateSeqNr::from(30)),
            Contact::new(Path::from([NodeId::with_msb(42)]), StateSeqNr::from(42)),
        ];

        let source_route = SourceRoute::from(Path::from([neighbor_id.clone(), root_id.clone()]));
        let event = UseCaseEvent::Message(
            PNDiscRsp(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                source_route,
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            assert_eq!(
                saved_contact.path(),
                &Path::from([neighbor_id.clone(), saved_contact.id().clone()])
            );
        }
    }

    #[test]
    fn extract_infos_from_query_route_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(15);

        let port = Port::new(String::from("test"));

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), port.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let neighbors_neighbors = vec![
            // Insertions strategy doesn't use cycle_remover or shortener. So the shared neighbor
            // will be included as all the others. Therefore removing it to avoid confusion
            //Contact::new(Path::from([neighbor_id.clone()]), StateSeqNr::from(15)),
            Contact::new(Path::from([NodeId::with_msb(30)]), StateSeqNr::from(30)),
            Contact::new(Path::from([NodeId::with_msb(42)]), StateSeqNr::from(42)),
        ];

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced();
        let event = UseCaseEvent::Message(
            QueryRouteRsp(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                source_route,
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            assert_eq!(
                saved_contact.path(),
                &Path::from([neighbor_id.clone(), source_id.clone(), contact.id().clone()])
            );
        }
    }

    #[test]
    fn extract_infos_from_find_node_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(15);

        let port = Port::new(String::from("test"));

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), port.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let message_hub = ArcSyncInMemoryMessageHub::new();
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

        let mut use_case = MessageInfoExtraction::default();

        let neighbors_neighbors = vec![
            // Insertions strategy doesn't use cycle_remover or shortener. So the shared neighbor
            // will be included as all the others. Therefore removing it to avoid confusion
            //Contact::new(Path::from([neighbor_id.clone()]), StateSeqNr::from(15)),
            Contact::new(
                Path::from([
                    NodeId::with_msb(16),
                    NodeId::with_msb(48),
                    NodeId::with_msb(30),
                ]),
                StateSeqNr::from(30),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(16), NodeId::with_msb(48)]),
                StateSeqNr::from(48),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(16), NodeId::with_msb(42)]),
                StateSeqNr::from(42),
            ),
        ];

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            NodeId::with_msb(120),
            NodeId::with_msb(127),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced()
        .advanced()
        .advanced();
        let event = UseCaseEvent::Message(
            FindNodeRsp(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                source_route,
            }),
            port.clone(),
        );
        let handled_result = use_case.handle(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            let mut expected_path = Path::from([
                neighbor_id.clone(),
                NodeId::with_msb(127),
                NodeId::with_msb(120),
                source_id.clone(),
            ]);
            expected_path.extend(contact.path().clone());
            assert_eq!(saved_contact.path(), &expected_path);
        }
    }
}
