use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::Deref;
use tracing::{Level, instrument};

use crate::domain::{
    Contact, DEFAULT_BUCKET_SIZE, GroupingError, NodeId, NotVia, RoutingTable, StateSeqNr,
    ULNTable, UnderlayNeighborId, node_id,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ErrorData, FindNodeReqData, ProtocolMessage, RTableData, ReqRspMessage};
use crate::use_cases::{EventHandler, NeverError, UseCaseContext, UseCaseEvent, UseCaseRuntime};

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
        if config.shared_prefix_bits_grouping.get() > node_id::BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_bits_grouping.get(),
                id_size: node_id::BIT_SIZE,
            });
        }

        Ok(Self {
            _c: PhantomData,
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
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = NeverError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "handle_overlay_discovery",
        "handle_overlay_discovery",
        skip(self, context),
        fields(
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        if let UseCaseEvent::Message(ProtocolMessage::FindNodeReq(req), _) = event {
            if req.destination() != context.root_id() {
                return Ok(());
            }

            let number_of_neighbors = match usize::try_from(req.data.neighborhood.get()) {
                Ok(value) => value,
                Err(e) => {
                    log::warn!(
                        target: "handle_overlay_discovery",
                        "FindNodeReq requested more contacts as host architecture can address: {e}. Returning max value"
                    );
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
                        From::from(*context.uln_table().state_seq_nr()),
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
                        // best contact we know is further away from the target than we are
                        // so we send back an error message, since we can't make progress
                        self.build_error(
                            context.not_via().clone(),
                            req.clone(),
                            From::from(*context.uln_table().state_seq_nr()),
                        )
                    }
                }
                (true, _, false, None) => self.build_error(
                    context.not_via().clone(),
                    req.clone(),
                    From::from(*context.uln_table().state_seq_nr()),
                ),
                (false, _, true, _) => {
                    log::warn!(
                        target: "handle_overlay_discovery",
                        "Received FindNodeReq with 'exact=false' with us as target: {req:?}"
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
                        // report what we know since we can't make any more progress

                        let closest = closest.into_iter().map(|(_, contact)| contact).collect();

                        self.build_find_node_rsp(
                            context.not_via().clone(),
                            req.clone(),
                            From::from(*context.uln_table().state_seq_nr()),
                            closest,
                        )
                    }
                }
                (false, _, false, None) => self.build_find_node_rsp(
                    context.not_via().clone(),
                    req.clone(),
                    From::from(*context.uln_table().state_seq_nr()),
                    Vec::with_capacity(0),
                ),
            };

            context
                .runtime()
                .send_message(outgoing_message, context.uln_table().deref());
        }

        Ok(())
    }
}
