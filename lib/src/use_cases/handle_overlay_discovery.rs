use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::context::UseCaseContext;
use crate::domain::{
    node_id, Contact, GroupingError, RoutingTable, StateSeqNr, DEFAULT_BUCKET_SIZE,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    ErrorData, FindNodeReqData, ProtocolMessage, ProtocolMessageSender, RTableData, ReqRspMessage,
};
use crate::use_cases::{EventHandler, MessageSentFailed, UseCaseEvent};

#[derive(Debug)]
pub struct OverlayDiscoveryConfig {
    pub shared_prefix_bits_grouping: NonZeroUsize,
}

impl Default for OverlayDiscoveryConfig {
    fn default() -> Self {
        Self {
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        }
    }
}

/// The shared functionality to answer to overlay discovery (`FindNodeReq`) appropriately.
///
/// As this is not a defined UseCase this is extracted as [EventHandler].
pub struct HandleOverlayDiscovery<C, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    _c: PhantomData<C>,
    config: OverlayDiscoveryConfig,
}

impl<C, const BUCKET_SIZE: usize> HandleOverlayDiscovery<C, BUCKET_SIZE> {
    /// Builds an instance of the [HandleOverlayDiscovery] and returns an error if the configuration
    /// is invalid.
    pub fn new(config: OverlayDiscoveryConfig) -> Result<Self, GroupingError> {
        if config.shared_prefix_bits_grouping.get() > node_id::BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_bits_grouping.get(),
                id_size: node_id::BIT_SIZE,
            });
        }

        Ok(Self {
            _c: PhantomData::default(),
            config,
        })
    }

    fn build_find_node_to_next_hop(
        &self,
        req: ReqRspMessage<FindNodeReqData>,
        next_contact: Contact,
    ) -> ProtocolMessage {
        let mut source_route = req.source_route;
        source_route.extend(next_contact.path().clone());
        source_route.advance();

        ProtocolMessage::FindNodeReq(ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: req.source_state_seq_nr,
            data: req.data,
            source_route,
        })
    }

    fn build_find_node_rsp(
        &self,
        req: ReqRspMessage<FindNodeReqData>,
        ssn: StateSeqNr,
        contacts: Vec<Contact>,
    ) -> ProtocolMessage {
        ProtocolMessage::FindNodeRsp(ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: ssn,
            data: RTableData { contacts },
            source_route: SourceRoute::from_reversed(req.source_route),
        })
    }

    fn build_error(&self, req: ReqRspMessage<FindNodeReqData>, ssn: StateSeqNr) -> ProtocolMessage {
        ProtocolMessage::Error(ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: ssn,
            data: ErrorData::DeadEnd,
            source_route: SourceRoute::from_reversed(req.source_route),
        })
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for HandleOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = MessageSentFailed;
    type Value = ();

    fn handle(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        if let UseCaseEvent::Message(ProtocolMessage::FindNodeReq(req), _) = event {
            if req.destination() != context.root_id() {
                return Ok(());
            }

            let number_of_neighbors = match usize::try_from(req.data.neighborhood.get()) {
                Ok(value) => value,
                Err(e) => {
                    log::warn!("FindNodeReq requested more contacts as host architecture can address: {}. Returning max value", e);
                    usize::MAX
                }
            };

            let closest = context
                .routing_table()
                .closest(
                    &req.data.target,
                    number_of_neighbors,
                    self.config.shared_prefix_bits_grouping.get(),
                )
                .expect("grouping has to be checked on initialization");

            // (exact, target, target is us, closest known)
            let outgoing_message = match (
                &req.data.exact,
                &req.data.target,
                &req.data.target == context.root_id(),
                closest.first(),
            ) {
                (true, _, true, _) => {
                    let closest = closest.into_iter().map(|(_, contact)| contact).collect();

                    self.build_find_node_rsp(
                        req.clone(),
                        *context.pn_table().state_seq_nr(),
                        closest,
                    )
                }
                (true, target, false, Some((closest_known_distance, contact))) => {
                    let own_distance = context
                        .root_id()
                        .shared_prefix_len(target, self.config.shared_prefix_bits_grouping.get())
                        .expect("grouping was checked on init");

                    if &own_distance > closest_known_distance && req.source() != contact.id() {
                        self.build_find_node_to_next_hop(req.clone(), Contact::clone(contact))
                    } else {
                        self.build_error(req.clone(), *context.pn_table().state_seq_nr())
                    }
                }
                (true, _, false, None) => {
                    self.build_error(req.clone(), *context.pn_table().state_seq_nr())
                }
                (false, _, true, _) => {
                    log::warn!(
                        "Received FindNodeReq with 'exact=false' with us as target: {:?}",
                        req
                    );
                    return Ok(());
                }
                (false, target, false, Some((closest_known_distance, contact))) => {
                    let own_distance = context
                        .root_id()
                        .shared_prefix_len(target, self.config.shared_prefix_bits_grouping.get())
                        .expect("grouping was checked on init");

                    if &own_distance > closest_known_distance && req.source() != contact.id() {
                        self.build_find_node_to_next_hop(req.clone(), Contact::clone(contact))
                    } else {
                        let closest = closest.into_iter().map(|(_, contact)| contact).collect();

                        self.build_find_node_rsp(
                            req.clone(),
                            *context.pn_table().state_seq_nr(),
                            closest,
                        )
                    }
                }
                (false, _, false, None) => self.build_find_node_rsp(
                    req.clone(),
                    *context.pn_table().state_seq_nr(),
                    Vec::with_capacity(0),
                ),
            };

            if let Err(e) = context.message_sender_mut().send(outgoing_message) {
                log::error!("Failed to send message: {}", e);
                return Err(MessageSentFailed);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU64, NonZeroUsize};

    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, InsertionStrategyResult, NetworkInterface, NodeId, PNTable, Path, RoutingTable,
        StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        ErrorData, FindNodeReqData, InMemoryMessageHub, Nonce, ProtocolMessage,
        ProtocolMessageReceiver, RTableData, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::handle_overlay_discovery::{
        HandleOverlayDiscovery, OverlayDiscoveryConfig,
    };
    use crate::use_cases::{EventHandler, UseCaseEvent};

    #[test]
    fn exact_target_returns_find_node_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let neighbor_id = NodeId::with_msb(3);
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), NetworkInterface::new("test"));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let config = OverlayDiscoveryConfig {
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        };
        let mut event_handler = HandleOverlayDiscovery::new(config).unwrap();

        let route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced()
        .advanced();

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: root_id.clone(),
            },
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageHub::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub.try_recv();
        assert!(
            sent_message.is_ok(),
            "Failed to receive message from hub: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message generated");
        let (sent_message, _) = sent_message.unwrap();

        if let ProtocolMessage::FindNodeRsp(rsp) = sent_message {
            assert_eq!(&rsp.nonce, &message.nonce, "Nonce not equal");
            assert_eq!(
                &rsp.source_state_seq_nr,
                &StateSeqNr::from(1),
                "SSN should be 1 as one neighbor was updated. But was: {}",
                rsp.source_state_seq_nr
            );
            assert_eq!(
                &rsp.source_route,
                &SourceRoute::from_reversed(route),
                "Source route should be reversed requests source route but was: {:?}",
                rsp.source_route
            );
            assert_eq!(
                &rsp.data,
                &RTableData {
                    contacts: vec![neighbor]
                },
                "Response should contain neighbor contact but was: {:?}",
                rsp.data
            );
        } else {
            panic!("generated invalid response: {:?}", sent_message);
        }
    }

    #[test]
    fn exact_and_known_contact_gets_delegated() {
        crate::tests::init();

        let root_id = NodeId::with_msb(10);
        let source_id = NodeId::with_msb(9);
        let neighbor_id = NodeId::with_msb(8);
        let target_id = NodeId::with_msb(0);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let target = Contact::new(
            Path::from([neighbor_id.clone(), target_id.clone()]),
            StateSeqNr::from(23),
        );

        let interface = NetworkInterface::new("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.insert(neighbor.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting neighbor failed: {:?}",
            insertion_result
        );
        let insertion_result = routing_table.insert(target.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting target failed: {:?}",
            insertion_result
        );

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        pn_table.insert(target_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let config = OverlayDiscoveryConfig {
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        };
        let mut event_handler = HandleOverlayDiscovery::new(config).unwrap();

        let route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced()
        .advanced();

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: target_id.clone(),
            },
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageHub::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub.try_recv();
        assert!(
            sent_message.is_ok(),
            "Failed to receive message from hub: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message generated");
        let (sent_message, _) = sent_message.unwrap();

        if let ProtocolMessage::FindNodeReq(rsp) = sent_message {
            assert_eq!(&rsp.nonce, &message.nonce, "Nonce not equal");
            assert_eq!(
                &rsp.source_state_seq_nr,
                &StateSeqNr::from(0),
                "SSN should be the same. But was: {}",
                rsp.source_state_seq_nr
            );
            assert_eq!(
                &rsp.data, &message.data,
                "Delegated data should be the same but was: {:?}",
                &rsp.data
            );
            let new_source = SourceRoute::from(Path::from([
                source_id.clone(),
                neighbor_id.clone(),
                root_id.clone(),
                neighbor_id.clone(),
                target_id.clone(),
            ]))
            .advanced()
            .advanced();
            assert_eq!(
                &rsp.source_route, &new_source,
                "Delegated request should have appropriate source route but was: {:?}",
                &rsp.source_route
            );
        } else {
            panic!("generated invalid response: {:?}", sent_message);
        }
    }

    #[test]
    fn exact_and_unknown_contact_returns_error() {
        crate::tests::init();

        let root_id = NodeId::with_msb(10);
        let source_id = NodeId::with_msb(9);
        let neighbor_id = NodeId::with_msb(8);
        let unknown_id = NodeId::with_msb(15);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let interface = NetworkInterface::new("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.insert(neighbor.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting neighbor failed: {:?}",
            insertion_result
        );

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let config = OverlayDiscoveryConfig {
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        };
        let mut event_handler = HandleOverlayDiscovery::new(config).unwrap();

        let route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced();

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: unknown_id.clone(),
            },
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageHub::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub.try_recv();
        assert!(
            sent_message.is_ok(),
            "Failed to receive message from hub: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message generated");
        let (sent_message, _) = sent_message.unwrap();

        if let ProtocolMessage::Error(rsp) = sent_message {
            assert_eq!(&rsp.nonce, &message.nonce, "Nonce not equal");
            assert_eq!(
                &rsp.source_state_seq_nr,
                &StateSeqNr::from(1),
                "SSN should be 1 as one neighbor was updated. But was: {}",
                rsp.source_state_seq_nr
            );
            assert_eq!(
                &rsp.source_route,
                &SourceRoute::from_reversed(route),
                "Source route should be reversed requests source route but was: {:?}",
                rsp.source_route
            );
            assert_eq!(
                &rsp.data,
                &ErrorData::DeadEnd,
                "Error should be dead end: {:?}",
                rsp.data
            );
        } else {
            panic!("generated invalid response: {:?}", sent_message);
        }
    }

    #[test]
    fn not_exact_and_known_closer_contact_is_delegated() {
        crate::tests::init();

        let root_id = NodeId::with_msb(10);
        let source_id = NodeId::with_msb(9);
        let neighbor_id = NodeId::with_msb(8);
        let closer_id = NodeId::with_msb(1);
        let target_id = NodeId::with_msb(0);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let closer_contact = Contact::new(
            Path::from([neighbor_id.clone(), closer_id.clone()]),
            StateSeqNr::from(13),
        );

        let interface = NetworkInterface::new("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.insert(neighbor.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting neighbor failed: {:?}",
            insertion_result
        );
        let insertion_result = routing_table.insert(closer_contact.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting closer contact failed: {:?}",
            insertion_result
        );

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let config = OverlayDiscoveryConfig {
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        };
        let mut event_handler = HandleOverlayDiscovery::new(config).unwrap();

        let route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced();

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: target_id.clone(),
            },
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageHub::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub.try_recv();
        assert!(
            sent_message.is_ok(),
            "Failed to receive message from hub: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message generated");
        let (sent_message, _) = sent_message.unwrap();

        if let ProtocolMessage::FindNodeReq(rsp) = sent_message {
            assert_eq!(&rsp.nonce, &message.nonce, "Nonce not equal");
            assert_eq!(
                &rsp.source_state_seq_nr, &message.source_state_seq_nr,
                "SSN should be same as received message. But was: {}",
                rsp.source_state_seq_nr
            );
            let new_source = SourceRoute::from(Path::from([
                source_id.clone(),
                neighbor_id.clone(),
                root_id.clone(),
                neighbor_id.clone(),
                closer_id.clone(),
            ]))
            .advanced()
            .advanced();
            assert_eq!(
                &rsp.source_route, &new_source,
                "Source route should be reversed requests source route but was: {:?}",
                rsp.source_route
            );
            assert_eq!(
                &rsp.data, &message.data,
                "Delegated data should be same as received data but was: {:?}",
                rsp.data
            );
        } else {
            panic!("generated invalid response: {:?}", sent_message);
        }
    }

    #[test]
    fn not_exact_and_no_closer_contact_returns_find_node_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(50);
        let neighbor_id = NodeId::with_msb(10);
        let target_id = NodeId::with_msb(0);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let interface = NetworkInterface::new("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.insert(neighbor.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting neighbor failed: {:?}",
            insertion_result
        );

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let config = OverlayDiscoveryConfig {
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        };
        let mut event_handler = HandleOverlayDiscovery::new(config).unwrap();

        let route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced();

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: target_id.clone(),
            },
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageHub::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub.try_recv();
        assert!(
            sent_message.is_ok(),
            "Failed to receive message from hub: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message generated");
        let (sent_message, _) = sent_message.unwrap();

        if let ProtocolMessage::FindNodeRsp(rsp) = sent_message {
            assert_eq!(&rsp.nonce, &message.nonce, "Nonce not equal");
            assert_eq!(
                &rsp.source_state_seq_nr,
                &StateSeqNr::from(1),
                "SSN should be the roots. But was: {}",
                rsp.source_state_seq_nr
            );
            assert_eq!(
                &rsp.source_route,
                &SourceRoute::from_reversed(message.source_route),
                "Source route should be reversed requests source route but was: {:?}",
                rsp.source_route
            );
            assert_eq!(
                &rsp.data,
                &RTableData {
                    contacts: vec![neighbor]
                },
                "Returned closest contacts: {:?}",
                rsp.data
            );
        } else {
            panic!("generated invalid response: {:?}", sent_message);
        }
    }
}
