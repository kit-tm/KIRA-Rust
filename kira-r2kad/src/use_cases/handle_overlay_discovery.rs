use std::{
    collections::HashMap,
    marker::PhantomData,
    num::NonZeroU8,
    ops::Deref,
};

use tracing::instrument;

use crate::{
    domain::{
        Contact,
        DEFAULT_BUCKET_SIZE,
        GroupingError,
        NodeId,
        NotViaList,
        ProtocolMessage,
        ProtocolMessageKind,
        RoutingTable,
        SourceRoute,
        StateSeqNr,
        ULNState,
        ULNTable,
        UnderlayNeighborId,
        protocol_message::{
            CommonHeader,
            ErrorData,
            FindNodeReqData,
            RTableData,
            ReqRspMessage,
            WireFormatMessage,
        },
    },
    use_cases::{
        EventHandler,
        NeverError,
        UseCaseContext,
        UseCaseEvent,
        UseCaseRuntime,
    },
};

#[derive(Debug)]
pub struct OverlayDiscoveryConfig {
    pub shared_prefix_bits_grouping: NonZeroU8,
}

impl Default for OverlayDiscoveryConfig {
    fn default() -> Self {
        Self {
            shared_prefix_bits_grouping: NonZeroU8::MIN,
        }
    }
}

/// The shared functionality to answer to overlay discovery (`FindNodeReq`) appropriately.
///
/// As this is not a defined UseCase this is extracted as [EventHandler].
#[derive(Debug)]
pub struct HandleOverlayDiscovery<C, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    _c: PhantomData<C>,
    config: OverlayDiscoveryConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for HandleOverlayDiscovery<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(OverlayDiscoveryConfig::default()).expect("Valid grouping with default config")
    }
}

impl<C, const BUCKET_SIZE: usize> HandleOverlayDiscovery<C, BUCKET_SIZE> {
    /// Builds an instance of the [HandleOverlayDiscovery] and returns an error if the configuration
    /// is invalid.
    pub fn new(config: OverlayDiscoveryConfig) -> Result<Self, GroupingError> {
        if config.shared_prefix_bits_grouping.get() > NodeId::BITS {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_bits_grouping,
            });
        }

        Ok(Self {
            _c: PhantomData,
            config,
        })
    }

    fn build_find_node_to_next_hop(
        &self,
        mut req: ReqRspMessage<FindNodeReqData>,
        next_contact: Contact,
    ) -> ProtocolMessage {
        req.source_route
            .extend(next_contact.path().unwrap().clone());
        req.source_route.advance();
        req.into()
    }

    fn build_find_node_rsp(
        &self,
        own_id: NodeId,
        not_via: Option<NotViaList>,
        req: ReqRspMessage<FindNodeReqData>,
        ssn: StateSeqNr,
        contacts: Vec<Contact>,
    ) -> ProtocolMessage {
        ProtocolMessage::FindNodeRsp(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeRsp,
                own_id,
                *req.src_node_id(),
                Some(req.msg_id()),
                ssn,
                req.src_node_degree() as usize,
            ),
            data: RTableData { contacts },
            not_via,
            source_route: SourceRoute::from_reversed(req.source_route),
        })
    }

    fn build_error(
        &self,
        own_id: NodeId,
        req: ReqRspMessage<FindNodeReqData>,
        ssn: StateSeqNr,
    ) -> ProtocolMessage {
        ProtocolMessage::Error(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::Error,
                own_id,
                *req.src_node_id(),
                Some(req.msg_id()),
                ssn,
                req.src_node_degree() as usize,
            ),
            data: ErrorData::DeadEnd,
            not_via: None,
            source_route: SourceRoute::from_reversed(req.source_route),
        })
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for HandleOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    type Context = C;
    type Error = NeverError;
    type Value = ();

    #[instrument(
        target = "handle_overlay_discovery",
        "handle_overlay_discovery",
        skip(self, context, event),
        fields(
            config = ?self.config,
            req.target = tracing::field::Empty,
            req.exact = tracing::field::Empty,
            req.source = tracing::field::Empty,
        )
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        let UseCaseEvent::Message(ProtocolMessage::FindNodeReq(req), _) = event else {
            return Ok(());
        };

        if req.destination() != context.root_id() {
            return Ok(());
        }

        let number_of_neighbors = match usize::try_from(req.data.neighborhood.get()) {
            Ok(value) => value,
            Err(e) => {
                tracing::warn!(
                    target: "handle_overlay_discovery",
                    "FindNodeReq requested more contacts as host architecture can address: {e}. Returning max value"
                );
                usize::MAX
            }
        };

        // collect closest nodes to target from routing table
        // closest returns only valid contacts, so unwrapping paths is safe
        let closest = context
            .routing_table()
            .closest(
                req.target(),
                number_of_neighbors,
                self.config.shared_prefix_bits_grouping,
            )
            .expect("grouping has to be checked on initialization");

        // Get any contact not contained in local or included not_via data
        // closest returns only valid contacts, so unwrapping paths is safe
        let closest_node = closest.first();

        let target = req.target();
        let target_is_us = target == context.root_id();
        let exact = req.exact();
        let req_source = req.source();

        // A response echoes the request's NotVia as its own information, so the ages have to
        // include the time since the requester sent them.
        let request_delay = context
            .routing_table()
            .one_way_delay(req_source)
            .unwrap_or_default();
        let echoed_not_via: Option<NotViaList> = req.not_via.as_ref().map(|not_via| {
            not_via
                .iter()
                .map(|entry| entry.aged(request_delay))
                .collect()
        });

        let span = tracing::Span::current();
        if !span.is_disabled() {
            span.record("req.target", format!("{target}"));
            span.record("req.exact", exact);
            span.record("req.source", format!("{req_source}"));
        }

        let outgoing_message = match (exact, target, target_is_us, &closest_node) {
            // === Exact ===
            // We are the target node
            (true, _, true, _) => {
                tracing::debug!(
                    target: "handle_overlay_discovery",
                    reason = "we are the target node",
                    "Returning routing table state"
                );
                let closest = closest.into_iter().map(|(_, contact)| contact).collect();

                self.build_find_node_rsp(
                    *context.root_id(),
                    echoed_not_via.clone(), // use the NotVia from the request also for the response
                    req.clone(),
                    From::from(*context.uln_table().state_seq_nr()),
                    closest,
                )
            }
            // We are not the target and found a closest contact
            (true, target, false, Some((closest_known_distance, contact))) => {
                let own_distance = context
                    .root_id()
                    .shared_prefix_len(target, self.config.shared_prefix_bits_grouping)
                    .expect("grouping was checked on init");

                if &own_distance > closest_known_distance && req.source() != contact.id() {
                    tracing::debug!(
                        target: "handle_overlay_discovery",
                        ?own_distance,
                        ?closest_known_distance,
                        next_hop = ?contact.id(),
                        "Forwarding exact FindNodeReq to closer overlay contact"
                    );
                    self.build_find_node_to_next_hop(req.clone(), Contact::clone(contact))
                } else {
                    // best contact we know is further away from the target than we are
                    // so we send back an error message, since we can't make progress
                    tracing::debug!(
                        target: "handle_overlay_discovery",
                        ?own_distance,
                        ?closest_known_distance,
                        reason = "Unable to make progress: Best known contact is further or equal",
                        "Building error response"
                    );
                    self.build_error(
                        *context.root_id(),
                        req.clone(),
                        From::from(*context.uln_table().state_seq_nr()),
                    )
                }
            }
            // We are not the target and did not find a closest contact
            (true, _, false, None) => {
                tracing::debug!(
                    target: "handle_overlay_discovery",
                    reason = "Exact FindNodeReq, not target, and no closest contact found",
                    "Building error response"
                );
                self.build_error(
                    *context.root_id(),
                    req.clone(),
                    From::from(*context.uln_table().state_seq_nr()),
                )
            }
            // === Non-Exact ===
            // Own NodeID as target
            (false, _, true, _) => {
                // This is unusual because non-exact FindNodeReq are usually
                // send to targets not existing in the network.
                //
                // Maybe we just got lucky.
                tracing::warn!(
                    target: "handle_overlay_discovery",
                    "Received non-exact FindNodeReq with us as target"
                );

                tracing::debug!(
                    target: "handle_overlay_discovery",
                    reason = "we are the target node",
                    "Returning routing table state"
                );
                let closest = closest.into_iter().map(|(_, contact)| contact).collect();
                self.build_find_node_rsp(
                    *context.root_id(),
                    echoed_not_via.clone(), // use the NotVia from the request also for the response
                    req.clone(),
                    From::from(*context.uln_table().state_seq_nr()),
                    closest,
                )
            }
            // We have a closest contact
            (false, target, false, Some((closest_known_distance, contact))) => {
                let own_distance = context
                    .root_id()
                    .shared_prefix_len(target, self.config.shared_prefix_bits_grouping)
                    .expect("grouping was checked on init");

                // forward FindNodeReq to closer contact
                if &own_distance > closest_known_distance && req.source() != contact.id() {
                    tracing::debug!(
                        target: "handle_overlay_discovery",
                        ?own_distance,
                        ?closest_known_distance,
                        next_hop = ?contact.id(),
                        "Forwarding non-exact FindNodeReq to closer contact"
                    );
                    self.build_find_node_to_next_hop(req.clone(), Contact::clone(contact))
                } else {
                    tracing::debug!(
                        target: "handle_overlay_discovery",
                        reason = "No progress possible for non-exact request",
                        "Returning routing table state"
                    );

                    let closest = closest.into_iter().map(|(_, contact)| contact).collect();
                    self.build_find_node_rsp(
                        *context.root_id(),
                        echoed_not_via.clone(),
                        req.clone(),
                        From::from(*context.uln_table().state_seq_nr()),
                        closest,
                    )
                }
            }
            // No closest contact found, sends back an empty FindNodeRsp
            (false, _, false, None) => {
                tracing::debug!(
                    target: "handle_overlay_discovery",
                    "Non-exact FindNodeReq with no closest contact; returning empty response"
                );

                self.build_find_node_rsp(
                    *context.root_id(),
                    echoed_not_via.clone(),
                    req.clone(),
                    From::from(*context.uln_table().state_seq_nr()),
                    Vec::with_capacity(0),
                )
            }
        };

        tracing::trace!(
            target: "handle_overlay_discovery",
            message = ?outgoing_message,
            "Sending outgoing message via runtime"
        );
        context.runtime().send_message(
            outgoing_message,
            context.uln_table().deref(),
            context.root_id(),
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroU64,
        time::Duration,
    };

    use super::*;
    use crate::{
        Output,
        context::{
            ContextConfig,
            SyncContext,
        },
        domain::{
            Age,
            ConnectionId,
            InterfaceId,
            Link,
            NotVia,
            Path,
            SafeStateSeqNr,
            SourceRoute,
            UnderlayNeighborSource,
            protocol_message::FindNodeReqData,
            routing_table::SingleBucketRT,
            underlay_neighbor_table::InMemoryULNTable,
        },
        runtime::testing::TestingUseCaseRuntime,
    };

    const BUCKET_SIZE: usize = 20;
    type Ctx =
        SyncContext<SingleBucketRT<BUCKET_SIZE>, TestingUseCaseRuntime, (), InMemoryULNTable, ()>;

    fn root() -> NodeId {
        NodeId::with_msb(1)
    }
    /// requester, an underlay neighbor
    fn requester() -> NodeId {
        NodeId::with_msb(2)
    }
    fn failed_link() -> Link {
        Link::new(NodeId::with_msb(3), NodeId::with_msb(4))
    }

    /// Context with the requester as contact, optionally with a measured RTT.
    fn context(srtt: Option<Duration>) -> Ctx {
        crate::tests::init();
        let ctx = Ctx::new(ContextConfig {
            root_id: root(),
            routing_table: SingleBucketRT::<BUCKET_SIZE>::new(root()),
            runtime: TestingUseCaseRuntime::default(),
            insertion_strategy: (),
            uln_table: InMemoryULNTable::new(),
            vicinity_graph: (),
        });
        ctx.uln_table_mut().insert(
            requester(),
            UnderlayNeighborId {
                interface_id: InterfaceId::try_from(1).unwrap(),
                connection_id: ConnectionId(0),
            },
        );
        let mut contact = Contact::new(
            Path::from(requester()),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        if let Some(srtt) = srtt {
            contact.path_mut().unwrap().record_rtt(srtt);
        }
        ctx.routing_table_mut().add(contact).unwrap();
        ctx
    }

    /// Exact FindNodeReq for us, carrying a NotVia entry that is `age_ms` old.
    fn find_node_req(age_ms: u64) -> UseCaseEvent {
        // arrived at us, the current hop
        let source_route = SourceRoute::new(requester(), Path::from(root()));
        assert_eq!(source_route.current_hop(), &root());
        let mut header = CommonHeader::new(
            ProtocolMessageKind::FindNodeReq,
            requester(),
            root(),
            Some(crate::domain::Nonce::random()),
            StateSeqNr::Value(SafeStateSeqNr::try_from(1).unwrap()),
            1,
        );
        *header.msg_flags_mut() |= crate::domain::protocol_message::ProtocolMessageFlags::Exact;
        UseCaseEvent::Message(
            ProtocolMessage::FindNodeReq(ReqRspMessage {
                common_header: header,
                data: FindNodeReqData {
                    neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                },
                not_via: Some(NotViaList::from([NotVia::from((
                    failed_link(),
                    Age::from(age_ms),
                ))])),
                source_route,
            }),
            UnderlayNeighborSource::Local,
        )
    }

    fn echoed_age(ctx: &Ctx) -> Age {
        let rsp = ctx
            .runtime()
            .output()
            .find_map(|output| match output {
                Output::SendProtocolMessage(ProtocolMessage::FindNodeRsp(rsp), _) => Some(rsp),
                _ => None,
            })
            .expect("FindNodeRsp sent");
        let not_via = rsp.not_via.expect("NotVia echoed");
        not_via
            .iter()
            .find(|entry| entry.link == failed_link())
            .expect("failed link echoed")
            .age
    }

    #[test]
    fn echoed_not_via_includes_request_transit_time() {
        // 100ms one-way delay from the requester
        let ctx = context(Some(Duration::from_millis(200)));
        let mut use_case = HandleOverlayDiscovery::<Ctx, BUCKET_SIZE>::default();

        use_case.handle_event(&ctx, find_node_req(100)).unwrap();

        assert_eq!(echoed_age(&ctx), Age::from(200));
    }

    #[test]
    fn echoed_not_via_without_rtt_estimate_keeps_age() {
        let ctx = context(None);
        let mut use_case = HandleOverlayDiscovery::<Ctx, BUCKET_SIZE>::default();

        use_case.handle_event(&ctx, find_node_req(100)).unwrap();

        assert_eq!(echoed_age(&ctx), Age::from(100));
    }
}
