use std::collections::HashSet;
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::context::UseCaseContext;
use crate::domain::{node_id, Contact, GroupingError, NotVia, RoutingTable, StateSeqNr, DEFAULT_BUCKET_SIZE, PNTable};
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
        not_via: HashSet<NotVia>,
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
            not_via,
            source_route,
        })
    }

    fn build_find_node_rsp(
        &self,
        not_via: HashSet<NotVia>,
        req: ReqRspMessage<FindNodeReqData>,
        ssn: StateSeqNr,
        contacts: Vec<Contact>,
    ) -> ProtocolMessage {
        ProtocolMessage::FindNodeRsp(ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: ssn,
            data: RTableData { contacts },
            not_via,
            source_route: SourceRoute::from_reversed(req.source_route),
        })
    }

    fn build_error(
        &self,
        not_via: HashSet<NotVia>,
        req: ReqRspMessage<FindNodeReqData>,
        ssn: StateSeqNr,
    ) -> ProtocolMessage {
        ProtocolMessage::Error(ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: ssn,
            data: ErrorData::DeadEnd,
            not_via,
            source_route: SourceRoute::from_reversed(req.source_route),
        })
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for HandleOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::PhysicalNeighborTable: PNTable
{
    type Context = C;
    type Error = MessageSentFailed;
    type Value = ();

    fn handle_event(
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

            // Get any contact not contained in local or included not_via data
            let closest_node = closest.iter().find(|(_, contact)| {
                if context.not_via().iter().any(|not_via| match not_via {
                    NotVia::Link(link) => contact.path().contains_link(link),
                }) {
                    return false;
                }
                if req.not_via.iter().any(|not_via| match not_via {
                    NotVia::Link(link) => contact.path().contains_link(link),
                }) {
                    return false;
                }

                true
            });

            // (exact, target, target is us, closest known)
            let outgoing_message = match (
                &req.data.exact,
                &req.data.target,
                &req.data.target == context.root_id(),
                &closest_node,
            ) {
                (true, _, true, _) => {
                    let closest = closest.into_iter().map(|(_, contact)| contact).collect();

                    self.build_find_node_rsp(
                        context.not_via().clone(),
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
                        self.build_find_node_to_next_hop(
                            context.not_via().clone(),
                            req.clone(),
                            Contact::clone(contact),
                        )
                    } else {
                        self.build_error(
                            context.not_via().clone(),
                            req.clone(),
                            *context.pn_table().state_seq_nr(),
                        )
                    }
                }
                (true, _, false, None) => self.build_error(
                    context.not_via().clone(),
                    req.clone(),
                    *context.pn_table().state_seq_nr(),
                ),
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
                        self.build_find_node_to_next_hop(
                            context.not_via().clone(),
                            req.clone(),
                            Contact::clone(contact),
                        )
                    } else {
                        let closest = closest.into_iter().map(|(_, contact)| contact).collect();

                        self.build_find_node_rsp(
                            context.not_via().clone(),
                            req.clone(),
                            *context.pn_table().state_seq_nr(),
                            closest,
                        )
                    }
                }
                (false, _, false, None) => self.build_find_node_rsp(
                    context.not_via().clone(),
                    req.clone(),
                    *context.pn_table().state_seq_nr(),
                    Vec::with_capacity(0),
                ),
            };

            if let Err(e) = context.message_sender_mut().send_message(outgoing_message) {
                log::error!("Failed to send message: {}", e);
                return Err(MessageSentFailed);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::{NonZeroU64, NonZeroUsize};

    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{Contact, InsertionStrategyResult, Link, NetworkInterface, NodeId, NotVia, Path, RoutingTable, StateSeqNr, TestInsertionStrategy, InMemoryPNTable};
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, ErrorData, FindNodeReqData, InMemoryMessageChannel, Nonce,
        ProtocolMessage, RTableData, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::handle_overlay_discovery::{
        HandleOverlayDiscovery, OverlayDiscoveryConfig,
    };
    use crate::use_cases::{EventHandler, UseCaseEvent};

    #[tokio::test]
    async fn exact_target_returns_find_node_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor_id = NodeId::with_msb(3);
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), NetworkInterface::with_name("test"));

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

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
            not_via: Default::default(),
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle_event(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageChannel::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
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

    #[tokio::test]
    async fn exact_and_known_contact_gets_delegated() {
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

        let interface = NetworkInterface::with_name("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

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

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        pn_table.insert(target_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

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
            not_via: Default::default(),
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle_event(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageChannel::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
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

    #[tokio::test]
    async fn exact_and_unknown_contact_returns_error() {
        crate::tests::init();

        let root_id = NodeId::with_msb(10);
        let source_id = NodeId::with_msb(9);
        let neighbor_id = NodeId::with_msb(8);
        let unknown_id = NodeId::with_msb(15);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let interface = NetworkInterface::with_name("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.insert(neighbor.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting neighbor failed: {:?}",
            insertion_result
        );

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

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
            not_via: Default::default(),
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle_event(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageChannel::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
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

    #[tokio::test]
    async fn not_exact_and_known_closer_contact_is_delegated() {
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

        let interface = NetworkInterface::with_name("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

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

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

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
            not_via: Default::default(),
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle_event(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageChannel::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
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

    #[tokio::test]
    async fn not_exact_and_no_closer_contact_returns_find_node_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(50);
        let neighbor_id = NodeId::with_msb(10);
        let target_id = NodeId::with_msb(0);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let interface = NetworkInterface::with_name("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.insert(neighbor.clone());
        assert!(
            insertion_result.is_ok(),
            "Inserting neighbor failed: {:?}",
            insertion_result
        );

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

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
            not_via: Default::default(),
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle_event(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageChannel::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
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

    #[tokio::test]
    async fn find_node_not_redirected_through_local_not_via_data() {
        crate::tests::init();

        // Topology:
        // root -- neighbor -- not_via -- target
        //                 `--   via   --´
        // root knowns neighbor, not_via and via.
        //
        // "via" is id wise closest to target
        // Expected: FindNode gets redirected to contact "via"

        let root_id = NodeId::with_msb(32);
        let source_id = NodeId::with_msb(16);
        let neighbor_id = NodeId::with_msb(8);
        let not_via_id = NodeId::with_msb(4);
        let via_id = NodeId::with_lsb(2);
        let target_id = NodeId::with_lsb(1);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(4));
        let not_via_contact = Contact::new(
            Path::from([neighbor_id.clone(), not_via_id.clone()]),
            StateSeqNr::from(8),
        );
        let via_contact = Contact::new(
            Path::from([neighbor_id.clone(), via_id.clone()]),
            StateSeqNr::from(16),
        );
        let neighbor_contact = Contact::new(
            Path::from([neighbor_id.clone(), not_via_id.clone(), target_id.clone()]),
            StateSeqNr::from(16),
        );

        let interface = NetworkInterface::with_name("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.extend(
            true,
            [
                neighbor.clone(),
                not_via_contact.clone(),
                via_contact.clone(),
                neighbor_contact.clone(),
            ],
        );
        assert!(
            insertion_result.is_ok(),
            "Inserting a contact failed: {:?}",
            insertion_result
        );

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let mut not_via = HashSet::new();
        not_via.insert(NotVia::Link(Link::new(neighbor_id.clone(), not_via_id)));

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via,
        });

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
            source_state_seq_nr: StateSeqNr::from(32),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: target_id.clone(),
            },
            not_via: HashSet::default(),
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle_event(
            &sync_context,
            UseCaseEvent::Message(
                message.clone().into(),
                InMemoryMessageChannel::dummy_interface(),
            ),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
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
                &StateSeqNr::from(32),
                "SSN should be the roots. But was: {}",
                rsp.source_state_seq_nr
            );
            assert_eq!(
                &rsp.source_route,
                &SourceRoute::from(Path::from([
                    source_id,
                    neighbor_id.clone(),
                    root_id,
                    neighbor_id,
                    via_id
                ]))
                .advanced()
                .advanced(),
                "Source route should be extended to not_via contact but was: {:?}",
                rsp.source_route
            );
            assert_eq!(
                &rsp.data,
                &FindNodeReqData {
                    exact: true,
                    neighborhood: NonZeroU64::new(20).unwrap(),
                    target: target_id,
                },
                "Returned closest contacts: {:?}",
                rsp.data
            );
        } else {
            panic!("generated invalid request: {:?}", sent_message);
        }
    }

    #[tokio::test]
    async fn find_node_not_redirected_through_included_not_via_data() {
        crate::tests::init();

        // Topology:
        // root -- neighbor -- not_via -- target
        //                 `--   via   --´
        // root knowns neighbor, not_via and via.
        //
        // "via" is id wise closest to target
        // Expected: FindNode gets redirected to contact "via"

        let root_id = NodeId::with_msb(32);
        let source_id = NodeId::with_msb(16);
        let neighbor_id = NodeId::with_msb(8);
        let not_via_id = NodeId::with_msb(4);
        let via_id = NodeId::with_lsb(2);
        let target_id = NodeId::with_lsb(1);

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(4));
        let not_via_contact = Contact::new(
            Path::from([neighbor_id.clone(), not_via_id.clone()]),
            StateSeqNr::from(8),
        );
        let via_contact = Contact::new(
            Path::from([neighbor_id.clone(), via_id.clone()]),
            StateSeqNr::from(16),
        );
        let target_contact = Contact::new(
            Path::from([neighbor_id.clone(), not_via_id.clone(), target_id.clone()]),
            StateSeqNr::from(16),
        );

        let interface = NetworkInterface::with_name("test");

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<10>::new(root_id.clone());
        let insertion_result = routing_table.extend(
            true,
            [
                neighbor.clone(),
                not_via_contact.clone(),
                via_contact.clone(),
                target_contact.clone(),
            ],
        );
        assert!(
            insertion_result.is_ok(),
            "Inserting a contact failed: {:?}",
            insertion_result
        );

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::new(),
        });

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

        let mut not_via = HashSet::new();
        not_via.insert(NotVia::Link(Link::new(
            neighbor_id.clone(),
            not_via_id.clone(),
        )));

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(32),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: target_id.clone(),
            },
            not_via,
            source_route: route.clone(),
        };
        let handle_result = event_handler.handle_event(
            &sync_context,
            UseCaseEvent::Message(message.clone().into(), interface.clone()),
        );
        assert!(
            handle_result.is_ok(),
            "Handling returned error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
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
                &StateSeqNr::from(32),
                "SSN should be the roots. But was: {}",
                rsp.source_state_seq_nr
            );
            assert_eq!(
                &rsp.source_route,
                &SourceRoute::from(Path::from([
                    source_id,
                    neighbor_id.clone(),
                    root_id,
                    neighbor_id,
                    via_id
                ]))
                .advanced()
                .advanced(),
                "Source route should be extended to not_via contact but was: {:?}",
                rsp.source_route
            );
            assert_eq!(
                &rsp.data,
                &FindNodeReqData {
                    exact: true,
                    neighborhood: NonZeroU64::new(20).unwrap(),
                    target: target_id,
                },
                "Returned closest contacts: {:?}",
                rsp.data
            );
        } else {
            panic!("generated invalid request: {:?}", sent_message);
        }
    }
}
