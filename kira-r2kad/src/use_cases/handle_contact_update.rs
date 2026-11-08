use std::{
    collections::HashMap,
    marker::PhantomData,
    mem,
    num::{
        NonZeroU8,
        NonZeroUsize,
    },
    ops::Deref,
    time::Duration,
};

use tracing::instrument;

use crate::{
    domain::{
        Contact,
        ContactState,
        Link,
        NodeId,
        NotViaState,
        NotViaStateList,
        ProtocolMessageKind,
        RoutingTable,
        SourceRoute,
        Timestamp,
        ULNState,
        ULNTable,
        UnderlayNeighborId,
        protocol_message::{
            CommonHeader,
            ContactActions,
            RouteUpdateActionType,
            UpdateRouteReq,
        },
    },
    use_cases::{
        ContactEvent,
        EventHandler,
        NeverError,
        TimerId,
        UseCaseContext,
        UseCaseEvent,
        UseCaseRuntime,
    },
};

/// Default of [HandleContactUpdateConfig::urgent_hold_time] (`UrgentUpdateHoldTime`).
pub const URGENT_UPDATE_HOLD_TIME: Duration = Duration::from_millis(200);
/// Default of [HandleContactUpdateConfig::normal_hold_time] (`NormalUpdateHoldTime`).
pub const NORMAL_UPDATE_HOLD_TIME: Duration = Duration::from_millis(500);

/// Configuration for contact update handling.
#[derive(Debug)]
pub struct HandleContactUpdateConfig {
    /// Number of ID-wise (XOR metric) closest overlay neighbors to notify of a change to the
    /// routing table, four as default
    pub notified_overlay_neighbors: NonZeroUsize,
    /// Number of bits to group for determining the closeness of contacts
    pub grouping_bits: NonZeroU8,
    /// Mean time to hold back updates that include an unreachable contact.
    pub urgent_hold_time: Duration,
    /// Mean time to hold back all other updates.
    pub normal_hold_time: Duration,
}

impl Default for HandleContactUpdateConfig {
    fn default() -> Self {
        HandleContactUpdateConfig {
            notified_overlay_neighbors: NonZeroUsize::new(4).unwrap(),
            grouping_bits: NonZeroU8::MIN,
            urgent_hold_time: URGENT_UPDATE_HOLD_TIME,
            normal_hold_time: NORMAL_UPDATE_HOLD_TIME,
        }
    }
}

/// Updates of a contact that wait to be sent.
#[derive(Debug, Default)]
struct PendingUpdate {
    /// Loss of the contact (Unreachable or WithDraw) with the contact at that time.
    loss: Option<(Contact, RouteUpdateActionType)>,
    /// Latest positive update after the loss (Announce or Change).
    ///
    /// The contact is taken from the routing table when sending, so its latest path is reported.
    gain: Option<RouteUpdateActionType>,
}

/// EventHandler for the common behavior to send UpdateRouteReq if a contacts path gets updated.
///
/// Updates are held back so that paths can converge and multiple
/// updates are aggregated into one UpdateRouteReq: urgent updates (an unreachable contact) for
/// `RandTime(urgent_hold_time)`, all others for `RandTime(normal_hold_time)`. When either time
/// elapses, all pending updates are sent. Per contact, the loss of the contact is reported
/// followed by its latest positive change, which is checked for consistency at sending time.
///
/// Updates not via data if contact gets invalid or gets valid again.
///
/// Also handles cascading effects.
/// If one contact gets removed or invalid, all contacts whose paths go through this contact
/// have to be invalidated.
#[derive(Debug)]
pub struct HandleContactUpdate<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    config: HandleContactUpdateConfig,
    /// Pending updates in the order the contacts changed first
    pending: Vec<(NodeId, PendingUpdate)>,
    urgent_timer: Option<TimerId>,
    normal_timer: Option<TimerId>,
}

impl<C, const BUCKET_SIZE: usize> Default for HandleContactUpdate<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<C, const BUCKET_SIZE: usize> HandleContactUpdate<C, BUCKET_SIZE> {
    pub fn new(config: HandleContactUpdateConfig) -> Self {
        Self {
            _pd: PhantomData,
            config,
            pending: Vec::new(),
            urgent_timer: None,
            normal_timer: None,
        }
    }

    fn pending_mut(&mut self, node_id: &NodeId) -> &mut PendingUpdate {
        let index = match self.pending.iter().position(|(id, _)| id == node_id) {
            Some(index) => index,
            None => {
                self.pending.push((*node_id, PendingUpdate::default()));
                self.pending.len() - 1
            }
        };
        &mut self.pending[index].1
    }
}

impl<C, const BUCKET_SIZE: usize> HandleContactUpdate<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    /// Starts the timer to send the pending updates unless it is already running.
    fn schedule_sending(&mut self, context: &C, urgent: bool) {
        let (timer, hold_time) = if urgent {
            (&mut self.urgent_timer, self.config.urgent_hold_time)
        } else {
            (&mut self.normal_timer, self.config.normal_hold_time)
        };
        if timer.is_none() {
            *timer = Some(context.runtime().register_rand_timer(hold_time));
        }
    }

    /// Records the loss of the contact, which makes a prior positive update obsolete.
    fn record_loss(&mut self, context: &C, contact: Contact, action: RouteUpdateActionType) {
        log::trace!(target: "handle_contact_update", "Scheduling update {action:?} for {}", contact.id());
        let pending = self.pending_mut(contact.id());
        pending.loss = Some((contact, action));
        pending.gain = None;
        self.schedule_sending(context, action == RouteUpdateActionType::Unreachable);
    }

    /// Records a positive update of the contact, replacing a prior one.
    fn record_gain(&mut self, context: &C, node_id: &NodeId, action: RouteUpdateActionType) {
        log::trace!(target: "handle_contact_update", "Scheduling update {action:?} for {node_id}");
        let pending = self.pending_mut(node_id);
        // receivers that didn't learn about the contact yet still need an announcement
        if pending.gain != Some(RouteUpdateActionType::Announce) {
            pending.gain = Some(action);
        }
        self.schedule_sending(context, false);
    }

    /// Sends all pending updates in one UpdateRouteReq per overlay neighbor.
    ///
    /// The NotVia information of lost contacts is included to inform about the failed links.
    fn send_pending_updates(&mut self, context: &C) {
        for timer in [self.urgent_timer.take(), self.normal_timer.take()]
            .into_iter()
            .flatten()
        {
            // one of them fired, removing it is harmless
            context.runtime().remove_timer(timer);
        }

        let mut updates = ContactActions::new();
        let mut not_via = NotViaStateList::default();
        {
            let rt = context.routing_table();
            for (node_id, pending) in mem::take(&mut self.pending) {
                if let Some((contact, _)) = &pending.loss
                    && let ContactState::Invalid(lost_links) = contact.state()
                {
                    not_via.nvs_list.extend(lost_links.nvs_list.iter().cloned());
                }
                updates.extend(pending.loss);
                // a positive update is only consistent with a currently valid contact
                if let Some(action) = pending.gain
                    && let Some(contact) = rt.contact(&node_id)
                    && contact.is_valid()
                {
                    updates.push((contact.clone(), action));
                }
            }
        }
        if !updates.is_empty() {
            self.send_update(context, updates, not_via);
        }
    }

    /// Returns whether the update can't be used by the given overlay neighbor:
    /// it ignores updates about itself and paths that contain itself.
    fn useless_for(neighbor: &NodeId, contact: &Contact, action: RouteUpdateActionType) -> bool {
        let positive = matches!(
            action,
            RouteUpdateActionType::Announce | RouteUpdateActionType::Change
        );
        contact.id() == neighbor
            || (positive && contact.path().is_some_and(|p| p.contains(neighbor)))
    }

    /// Returns the `notified_overlay_neighbors` ID-wise closest overlay neighbors.
    fn closest_overlay_neighbors(&self, context: &C) -> Vec<Contact> {
        let root = context.root_id();
        // closest() may return more contacts with the same shared prefix length
        let mut neighbors: Vec<Contact> = context
            .routing_table()
            .closest(
                root,
                self.config.notified_overlay_neighbors.get(),
                self.config.grouping_bits,
            )
            .expect("type didn't prevent invalid grouping")
            .into_iter()
            .map(|(_, contact)| contact)
            .collect();
        neighbors.sort_by_key(|contact| contact.id() ^ root);
        neighbors.truncate(self.config.notified_overlay_neighbors.get());
        neighbors
    }

    fn send_update(&self, context: &C, updates: ContactActions, not_via: NotViaStateList) {
        for contact in self.closest_overlay_neighbors(context) {
            let contact_actions: ContactActions = updates
                .iter()
                .filter(|(updated, action)| !Self::useless_for(contact.id(), updated, *action))
                .cloned()
                .collect();
            if contact_actions.is_empty() {
                continue;
            }
            let message = UpdateRouteReq {
                common_header: CommonHeader::new(
                    ProtocolMessageKind::UpdateRouteReq,
                    *context.root_id(),
                    *contact.id(),
                    None,
                    *context.uln_table().state_seq_nr(),
                    context.uln_table().size(),
                ),
                not_via: From::from(not_via.clone()),
                contact_actions,
                source_route: SourceRoute::new(*context.root_id(), contact.path().unwrap().clone()),
            };

            log::trace!(target: "handle_contact_update", "Sending update to contact {}, msg: {:?}", contact.id(), message);

            context
                .runtime()
                .send_message(message, context.uln_table().deref(), context.root_id());
        }
    }

    // this should only be called for ULN contacts as invalidated_contact
    fn invalidate_all_affected_contacts(&self, context: &C, invalidated_contact: &Contact) {
        // check if invalidated_contact is a ULN
        if !invalidated_contact.is_uln() {
            return;
        }
        // Invalidate all other contacts via this contact
        for mut saved_contact in context.routing_table_mut().iter_mut() {
            if saved_contact.path().is_some()
                && saved_contact
                    .path()
                    .unwrap()
                    .starts_with(invalidated_contact.path().unwrap())
            {
                let nvs = NotViaState::new(
                    Link::new(*context.root_id(), *invalidated_contact.id()),
                    Timestamp::now(),
                );

                if let ContactState::Invalid(not_via_state_list) = invalidated_contact.state() {
                    // use not via info from invalidated contact
                    let mut nvs_list = not_via_state_list.clone();
                    nvs_list.nvs_list.insert(nvs);
                    saved_contact.set_invalid(nvs_list);
                } else {
                    // since invalidated_contact is a ULN, we use this info
                    saved_contact.set_invalid(NotViaStateList::from(nvs));
                }
            }
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for HandleContactUpdate<C, BUCKET_SIZE>
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
        target = "handle_contact_update",
        "handle_contact_update",
        skip(self, context, event),
        fields(
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        if let UseCaseEvent::Timer(timer_id) = event {
            if [self.urgent_timer, self.normal_timer].contains(&Some(timer_id)) {
                self.send_pending_updates(context);
            }
        } else if let UseCaseEvent::Contact(contact_event) = event {
            // handle contact events
            match *contact_event {
                ContactEvent::Removed(contact) => {
                    // only send update if underlay neighbor got removed
                    // this doesn't happen in reality since we use the UnlimitedULNRoutingTable
                    if contact.is_uln() {
                        log::trace!(target: "handle_contact_update", "Update concerning removal of underlay neighbor {}", contact.id());
                        self.record_loss(context, contact.clone(), RouteUpdateActionType::WithDraw);
                    }

                    self.invalidate_all_affected_contacts(context, &contact);

                    if context.uln_table_mut().remove(contact.id()).is_some() {
                        log::trace!(target: "handle_contact_update", "removed {} from underlay neighbors", contact.id());
                    }
                }
                ContactEvent::Updated { new, old } => {
                    if new.is_valid() {
                        // propagate changes of the active path, including a contact that got
                        // valid again ("good news")
                        let path_changed = match (new.path(), old.path()) {
                            (Some(new_path), Some(old_path)) => !new_path.is_same_path_as(old_path),
                            (new_path, old_path) => new_path.is_some() != old_path.is_some(),
                        };
                        if !old.is_valid() || path_changed {
                            self.record_gain(context, new.id(), RouteUpdateActionType::Change);
                        }
                    } else if old.is_valid() && old.is_uln() {
                        // announce only the loss of a direct underlay neighbor (receivers only use
                        // Unreachable for the link to it), and not every later change while it
                        // stays unusable (e.g., rediscovery state or a proposed path)
                        self.record_loss(context, *new.clone(), RouteUpdateActionType::Unreachable);
                    }

                    if old.is_valid() && !new.is_valid() {
                        // Add to not-via data if path gets invalid (maybe done already)
                        self.invalidate_all_affected_contacts(context, &new);
                    }
                }
                ContactEvent::New(new) => {
                    log::trace!(target: "handle_contact_update", "New contact {} found. Scheduling update for {:?}", new.id(), new);

                    // always send an update if a new contact was found
                    // TODO this is mostly useful only for ULNs
                    self.record_gain(context, new.id(), RouteUpdateActionType::Announce);
                }
                _ => {}
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Output,
        context::{
            ContextConfig,
            SyncContext,
        },
        domain::{
            ConnectionId,
            InterfaceId,
            NotVia,
            Path,
            ProtocolMessage,
            SafeStateSeqNr,
            routing_table::SingleBucketRT,
            underlay_neighbor_table::InMemoryULNTable,
            vicinity::vicinity_graph::PetVicinityGraph,
        },
        runtime::testing::TestingUseCaseRuntime,
    };

    const BUCKET_SIZE: usize = 20;
    type Ctx = SyncContext<
        SingleBucketRT<BUCKET_SIZE>,
        TestingUseCaseRuntime,
        (),
        InMemoryULNTable,
        PetVicinityGraph,
    >;
    type Hcu = HandleContactUpdate<Ctx, BUCKET_SIZE>;

    fn root() -> NodeId {
        NodeId::with_msb(1)
    }
    /// underlay neighbor via which updates are sent
    fn a() -> NodeId {
        NodeId::with_msb(2)
    }
    /// underlay neighbor whose changes are reported
    fn c() -> NodeId {
        NodeId::with_msb(3)
    }
    fn x() -> NodeId {
        NodeId::with_msb(4)
    }
    fn y() -> NodeId {
        NodeId::with_msb(5)
    }
    /// overlay neighbor behind [a] that receives the updates
    fn e() -> NodeId {
        NodeId::with_msb(6)
    }
    /// contact behind [a] that is no underlay neighbor
    fn d() -> NodeId {
        NodeId::with_msb(7)
    }
    fn ssn(value: u32) -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(value).unwrap()
    }
    fn valid_path(nodes: &[NodeId]) -> Path {
        let mut path = Path::try_from(nodes).unwrap();
        path.set_valid();
        path
    }
    fn lost_link() -> Link {
        Link::new(root(), c())
    }

    fn setup() -> (Hcu, Ctx) {
        crate::tests::init();

        let ctx = Ctx::new(ContextConfig {
            root_id: root(),
            routing_table: SingleBucketRT::<BUCKET_SIZE>::new(root()),
            runtime: TestingUseCaseRuntime::with_unique_timer_ids(),
            insertion_strategy: (),
            uln_table: InMemoryULNTable::new(),
            vicinity_graph: PetVicinityGraph::new(root()),
        });
        for (id, uln) in [(1, a()), (2, c())] {
            ctx.uln_table_mut().insert(
                uln,
                UnderlayNeighborId {
                    interface_id: InterfaceId::try_from(id).unwrap(),
                    connection_id: ConnectionId(id),
                },
            );
        }
        for nodes in [&[a()][..], &[c()], &[a(), e()], &[a(), d()]] {
            ctx.routing_table_mut()
                .add(Contact::new(valid_path(nodes), ssn(1)))
                .unwrap();
        }
        // e must be among the notified closest overlay neighbors
        let config = HandleContactUpdateConfig {
            notified_overlay_neighbors: NonZeroUsize::new(BUCKET_SIZE).unwrap(),
            ..Default::default()
        };
        (Hcu::new(config), ctx)
    }

    /// Changes the contact and hands the change to the use case.
    fn change(hcu: &mut Hcu, ctx: &Ctx, node: NodeId, change: impl FnOnce(&mut Contact)) {
        let old = ctx.routing_table().contact(&node).unwrap().clone();
        change(ctx.routing_table_mut().contact_mut(&node).unwrap());
        let new = ctx.routing_table().contact(&node).unwrap().clone();
        hcu.handle_event(
            ctx,
            UseCaseEvent::Contact(Box::new(ContactEvent::Updated {
                new: Box::new(new),
                old: Box::new(old),
            })),
        )
        .unwrap();
    }

    fn invalidate(hcu: &mut Hcu, ctx: &Ctx, node: NodeId, link: Link) {
        change(hcu, ctx, node, |contact| {
            contact.set_invalid(NotViaStateList::from(NotViaState::new(
                link,
                Timestamp::now(),
            )))
        });
    }

    fn fire(hcu: &mut Hcu, ctx: &Ctx, timer: TimerId) {
        hcu.handle_event(ctx, UseCaseEvent::Timer(timer)).unwrap();
    }

    type Entry = (NodeId, RouteUpdateActionType, Vec<NodeId>);

    /// UpdateRouteReqs sent since the last call: destination, updates and NotVia links
    fn sent_updates(ctx: &Ctx) -> HashMap<NodeId, (Vec<Entry>, Vec<Link>)> {
        let mut sent = HashMap::new();
        for output in ctx.runtime().output() {
            let Output::SendProtocolMessage(ProtocolMessage::UpdateRouteReq(req), _) = output
            else {
                continue;
            };
            let entries = req
                .contact_actions
                .into_iter()
                .map(|(contact, action)| {
                    let path = contact.path().unwrap().as_ref().to_vec();
                    (*contact.id(), action, path)
                })
                .collect();
            let links = req
                .not_via
                .unwrap_or_default()
                .into_iter()
                .map(|not_via: NotVia| not_via.link)
                .collect();
            let duplicate = sent.insert(*req.common_header.dest_id(), (entries, links));
            assert!(duplicate.is_none(), "one UpdateRouteReq per destination");
        }
        sent
    }

    fn sent_to_e(ctx: &Ctx) -> Option<Vec<Entry>> {
        sent_updates(ctx).remove(&e()).map(|(entries, _)| entries)
    }

    #[test]
    fn loss_and_latest_path_are_sent_together_after_urgent_hold_time() {
        let (mut hcu, ctx) = setup();

        invalidate(&mut hcu, &ctx, c(), lost_link());
        assert!(sent_updates(&ctx).is_empty(), "loss is held back");
        let urgent = hcu.urgent_timer.expect("urgent timer started");
        let (_, hold_time) = ctx.runtime().registered_timers()[0];
        assert!(
            (URGENT_UPDATE_HOLD_TIME / 2..=URGENT_UPDATE_HOLD_TIME * 3 / 2).contains(&hold_time),
            "RandTime(UrgentUpdateHoldTime), got {hold_time:?}"
        );

        // good news, then an even better path in quick succession
        change(&mut hcu, &ctx, c(), |contact| {
            assert!(contact.assess_path_candidate_and_update(&valid_path(&[a(), x(), c()])))
        });
        change(&mut hcu, &ctx, c(), |contact| {
            contact.set_path(valid_path(&[a(), y(), c()]))
        });
        assert!(sent_updates(&ctx).is_empty());
        let normal = hcu.normal_timer.expect("normal timer started");

        fire(&mut hcu, &ctx, urgent);
        let mut sent = sent_updates(&ctx);
        assert_eq!(
            sent.remove(&e()).expect("sent to e"),
            (
                vec![
                    (c(), RouteUpdateActionType::Unreachable, vec![c()]),
                    (c(), RouteUpdateActionType::Change, vec![a(), y(), c()]),
                ],
                vec![lost_link()],
            ),
            "loss with NotVia, followed by the latest path"
        );
        assert!(ctx.runtime().removed_timers().contains(&normal));

        // the other timer has nothing left to send
        fire(&mut hcu, &ctx, normal);
        assert!(sent_updates(&ctx).is_empty());
    }

    #[test]
    fn updates_are_not_sent_to_neighbors_that_ignore_them() {
        let (mut hcu, ctx) = setup();

        invalidate(&mut hcu, &ctx, c(), lost_link());
        change(&mut hcu, &ctx, c(), |contact| {
            assert!(contact.assess_path_candidate_and_update(&valid_path(&[a(), c()])))
        });
        let timer = hcu.urgent_timer.unwrap();
        fire(&mut hcu, &ctx, timer);

        let sent = sent_updates(&ctx);
        assert!(!sent.contains_key(&c()), "no updates about c to c itself");
        assert_eq!(
            sent[&a()].0,
            vec![(c(), RouteUpdateActionType::Unreachable, vec![c()])],
            "a gets no path via itself"
        );
        assert_eq!(sent[&e()].0.len(), 2, "e gets the loss and the new path");
    }

    #[test]
    fn updates_are_sent_only_to_closest_overlay_neighbors() {
        let (_, ctx) = setup();
        let mut hcu = Hcu::new(HandleContactUpdateConfig {
            notified_overlay_neighbors: NonZeroUsize::new(2).unwrap(),
            ..Default::default()
        });

        change(&mut hcu, &ctx, d(), |contact| {
            contact.set_path(valid_path(&[a(), x(), d()]))
        });
        let timer = hcu.normal_timer.unwrap();
        fire(&mut hcu, &ctx, timer);

        // c and a are ID-wise closest to the root, a ignores a path via itself
        let sent = sent_updates(&ctx);
        assert_eq!(sent.keys().copied().collect::<Vec<_>>(), vec![c()]);
    }

    #[test]
    fn loss_of_contact_that_is_no_underlay_neighbor_is_not_sent() {
        let (mut hcu, ctx) = setup();
        invalidate(&mut hcu, &ctx, d(), Link::new(a(), d()));
        assert!(hcu.pending.is_empty(), "receivers ignore Unreachable for d");
        assert!(hcu.urgent_timer.is_none() && hcu.normal_timer.is_none());
    }

    #[test]
    fn loss_makes_prior_path_change_obsolete() {
        let (mut hcu, ctx) = setup();

        change(&mut hcu, &ctx, c(), |contact| {
            contact.set_path(valid_path(&[a(), x(), c()]))
        });
        // the path is direct again, then the direct link fails
        change(&mut hcu, &ctx, c(), |contact| {
            contact.set_path(valid_path(&[c()]))
        });
        invalidate(&mut hcu, &ctx, c(), lost_link());

        let timer = hcu.urgent_timer.unwrap();
        fire(&mut hcu, &ctx, timer);
        assert_eq!(
            sent_to_e(&ctx).unwrap(),
            vec![(c(), RouteUpdateActionType::Unreachable, vec![c()])],
        );
    }

    #[test]
    fn change_is_sent_only_for_changed_path_after_normal_hold_time() {
        let (mut hcu, ctx) = setup();

        // same path, e.g., only a newer state sequence number
        change(&mut hcu, &ctx, d(), |contact| {
            contact.set_state_seq_nr(ssn(2))
        });
        assert!(hcu.pending.is_empty() && hcu.normal_timer.is_none());

        change(&mut hcu, &ctx, d(), |contact| {
            contact.set_path(valid_path(&[a(), x(), d()]))
        });
        change(&mut hcu, &ctx, d(), |contact| {
            contact.set_path(valid_path(&[a(), y(), d()]))
        });
        assert!(hcu.urgent_timer.is_none(), "no urgent update");
        assert!(sent_updates(&ctx).is_empty());

        let timer = hcu.normal_timer.unwrap();
        fire(&mut hcu, &ctx, timer);
        assert_eq!(
            sent_to_e(&ctx).unwrap(),
            vec![(d(), RouteUpdateActionType::Change, vec![a(), y(), d()])],
        );
    }

    #[test]
    fn new_contact_stays_announced_with_latest_path() {
        let (mut hcu, ctx) = setup();

        let new = ctx.routing_table().contact(&d()).unwrap().clone();
        hcu.handle_event(
            &ctx,
            UseCaseEvent::Contact(Box::new(ContactEvent::New(new))),
        )
        .unwrap();
        change(&mut hcu, &ctx, d(), |contact| {
            contact.set_path(valid_path(&[a(), x(), d()]))
        });

        let timer = hcu.normal_timer.unwrap();
        fire(&mut hcu, &ctx, timer);
        assert_eq!(
            sent_to_e(&ctx).unwrap(),
            vec![(d(), RouteUpdateActionType::Announce, vec![a(), x(), d()])],
        );
    }

    #[test]
    fn unreachable_is_recorded_only_when_contact_gets_invalid() {
        let (mut hcu, ctx) = setup();
        invalidate(&mut hcu, &ctx, c(), lost_link());
        let timer = hcu.urgent_timer.unwrap();
        fire(&mut hcu, &ctx, timer);
        assert_eq!(
            sent_to_e(&ctx).unwrap(),
            vec![(c(), RouteUpdateActionType::Unreachable, vec![c()])],
        );

        // rediscovery and a proposed path don't make the contact usable again
        change(&mut hcu, &ctx, c(), |contact| {
            let not_via = NotViaStateList::from(NotViaState::new(lost_link(), Timestamp::now()));
            contact.start_rediscovering(not_via, vec![], 5, Duration::from_millis(100));
            let mut proposed = Path::try_from(&[a(), x(), c()][..]).unwrap();
            proposed.set_notvalidated();
            assert!(contact.assess_path_candidate_and_update(&proposed));
        });
        assert!(hcu.pending.is_empty(), "no repeated bad news");
        assert!(hcu.urgent_timer.is_none() && hcu.normal_timer.is_none());
    }
}
