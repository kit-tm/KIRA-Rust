use std::{
    collections::HashMap,
    error::Error,
    marker::PhantomData,
    ops::Deref,
};

use derive_more::derive::Display;
use tracing::{
    field,
    instrument,
};

use crate::{
    domain::{
        Bucket,
        Contact,
        ContactState,
        Hasher,
        NodeId,
        NodeIdSubnet,
        NodeIdTableUpdate,
        Path,
        PathId,
        PathSegmentation,
        PathSegments,
        RoutingTable,
        ULNState,
        UnderlayNeighborId,
        UnderlayNeighborUpdate,
        VICINITY_RADIUS,
        protocol_event::forwarding::{
            DecapsulationDestination,
            NodeIdEncapsulationEntry,
            NodeIdEntry,
            NodeIdForwardingEntry,
            PathIdDecapsulationEntry,
            PathIdEntry,
            PathIdForwardingEntry,
        },
    },
    runtime::UseCaseRuntime,
    use_cases::{
        ContactEvent,
        EventHandler,
        UseCase,
        UseCaseContext,
        UseCaseEvent,
        UseCaseState,
    },
};

/// Configuration for [DeriveFwdTableEntries] use case.
#[derive(Debug, Default)]
pub struct DeriveFwdTableEntriesConfig {
    /// Hashing algorithm to use.
    pub hasher: Hasher,
    /// How paths to contacts are split into path segments.
    pub segmentation: PathSegmentation,
}

#[derive(Debug, Default)]
pub enum DeriveFwdTableEntriesState {
    #[default]
    /// [DeriveFwdTableEntries] is initialized.
    Initialized,
    Running {
        bucket_prefix_entries: HashMap<usize, NodeIdSubnet>,
        /// The contact each bucket prefix entry is derived from
        bucket_prefix_contacts: HashMap<usize, NodeId>,
        /// Entries of the PathId forwarding table used by the contacts, i.e.,
        /// the entries of the first segments of their paths.
        local_entries: HashMap<NodeId, PathIdEntry>,
    },
    /// [DeriveFwdTableEntries] reached an unrecoverable error state.
    Error,
}

impl UseCaseState for DeriveFwdTableEntriesState {
    fn is_error(&self) -> bool {
        matches!(self, Self::Initialized)
    }
}

/// Use case to derive forwarding table entries.
///
/// ## Invariants
///
/// - For every valid [Contact] in the [RoutingTable] whose path is
///   [established in the forwarding tier](Path::is_forwarding_established) a [NodeIdEntry] exists.
/// - For every such [Contact] which is not a underlay neighbor the [PathIdEntry] of the first
///   [segment](PathSegments) of its path is marked as locally used in the
///   [PathIdFwdState](crate::domain::PathIdFwdState).
#[derive(Debug, Default)]
pub struct DeriveFwdTableEntries<C, const BUCKET_SIZE: usize> {
    state: DeriveFwdTableEntriesState,
    config: DeriveFwdTableEntriesConfig,

    _pd: PhantomData<C>,
}

impl<C, const BUCKET_SIZE: usize> DeriveFwdTableEntries<C, BUCKET_SIZE> {
    pub fn new(config: DeriveFwdTableEntriesConfig) -> Self {
        Self {
            state: DeriveFwdTableEntriesState::default(),
            config,
            _pd: PhantomData,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    fn remove_node_id_entry(
        &mut self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<(), DeriveFwdEntriesError> {
        self.set_local_entry(context, node_id, None);

        let DeriveFwdTableEntriesState::Running {
            bucket_prefix_contacts,
            ..
        } = &mut self.state
        else {
            panic!("remove_node_id_entry called without running DeriveFwdTableEntries");
        };

        let entry = NodeIdSubnet::new(*node_id);
        tracing::trace!(target: "derive_fwd_table_entries", %entry, "remove entry");
        context
            .runtime()
            .update_fwd_tables(NodeIdTableUpdate::Remove(entry));

        let bucket_index = context.routing_table().get_bucket_index(node_id);

        // If contact not used as bucket prefix entry was removed
        // we don't need to recalculate the bucket prefix entry.
        //
        // The bucket prefix size doesn't change on contact removals.
        // We don't merge buckets.
        // (The NodeId of the bucket prefix is the prefix, not the contact it is derived from.)
        if bucket_prefix_contacts
            .get(&bucket_index)
            .is_some_and(|prefix_contact| prefix_contact != node_id)
        {
            return Ok(());
        }
        let _span = tracing::debug_span!(
            target: "derive_fwd_table_entries",
            "update_bucket_entries",
            bucket_index,
            reason = "contact_removal",
            kind = "NodeId",
        )
        .entered();
        self.update_bucket(context, bucket_index)?;

        Ok(())
    }

    fn create_node_id_entry(
        &mut self,
        context: &C,
        contact: Contact,
    ) -> Result<(), DeriveFwdEntriesError> {
        if !self.is_forwarding_established(&contact) {
            // the PathIds of the path still have to be set up in the nodes on the path
            tracing::trace!(target: "derive_fwd_table_entries", node = %contact.id(), "path not yet established in the forwarding tier");
            return self.remove_node_id_entry(context, contact.id());
        }

        let entry = self.derive_node_id_entry(context, &contact)?;
        let local_entry = self.derive_local_entry(context, &contact, &entry);
        self.set_local_entry(context, contact.id(), local_entry);
        tracing::trace!(target: "derive_fwd_table_entries", %entry, "create entry");
        context
            .runtime()
            .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(entry));

        // new contact could have shortest path in bucket (PNS)
        // or added to lowest bucket (XOR-metrics derived prefix entries).
        let bucket_index = context.routing_table().get_bucket_index(contact.id());
        let _span = tracing::debug_span!(
            target: "derive_fwd_table_entries",
            "update_bucket_entries",
            bucket_index,
            reason = "new_contact",
            kind = "NodeId",
        )
        .entered();
        self.update_bucket(context, bucket_index)?;

        Ok(())
    }

    fn update_node_id_entry(
        &mut self,
        context: &C,
        contact: Contact,
    ) -> Result<(), DeriveFwdEntriesError> {
        if !self.is_forwarding_established(&contact) {
            // the PathIds of the path still have to be set up in the nodes on the path
            tracing::trace!(target: "derive_fwd_table_entries", node = %contact.id(), "path not yet established in the forwarding tier");
            return self.remove_node_id_entry(context, contact.id());
        }

        let entry = self.derive_node_id_entry(context, &contact)?;
        let local_entry = self.derive_local_entry(context, &contact, &entry);
        self.set_local_entry(context, contact.id(), local_entry);
        tracing::trace!(target: "derive_fwd_table_entries", %entry, "update entry");
        context
            .runtime()
            .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(entry));

        // updated contact could have shortest path in bucket (PNS)
        // or added to lowest bucket (XOR-metrics derived prefix entries).
        let bucket_index = context.routing_table().get_bucket_index(contact.id());
        let _span = tracing::debug_span!(
            target: "derive_fwd_table_entries",
            "update_bucket_entries",
            bucket_index,
            reason = "contact_update",
            kind = "NodeId",
        )
        .entered();
        self.update_bucket(context, bucket_index)?;

        Ok(())
    }

    fn update_bucket(
        &mut self,
        context: &C,
        bucket_index: usize,
    ) -> Result<(), DeriveFwdEntriesError> {
        let rt = context.routing_table();
        let last_bucket_index = rt.get_bucket_index(context.root_id());

        // Derive entries purely by XOR-metrics in last bucket for key-based routing
        if bucket_index + 1 >= last_bucket_index {
            // TODO: Derive bucket prefix entries by XOR-metrics for key-based routing
        };

        // derive one prefix entry of the bucket using proximity neighbor selection
        let Some((prefix_entry, prefix_contact)) =
            self.derive_bucket_prefix_entry(context, bucket_index)?
        else {
            let DeriveFwdTableEntriesState::Running {
                bucket_prefix_entries,
                bucket_prefix_contacts,
                ..
            } = &mut self.state
            else {
                panic!("update_bucket called without running DeriveFwdTableEntries");
            };
            bucket_prefix_contacts.remove(&bucket_index);

            // remove old entry of bucket
            let Some(old_prefix) = bucket_prefix_entries.remove(&bucket_index) else {
                return Ok(());
            };

            tracing::debug!(
                target: "derive_fwd_table_entries",
                bucket_index,
                reason = "bucket_empty",
                kind = "NodeId",
                %old_prefix,
                "remove entry",
            );
            context
                .runtime()
                .update_fwd_tables(NodeIdTableUpdate::Remove(old_prefix));

            return Ok(());
        };

        let DeriveFwdTableEntriesState::Running {
            bucket_prefix_entries,
            bucket_prefix_contacts,
            ..
        } = &mut self.state
        else {
            panic!("update_bucket called without running DeriveFwdTableEntries");
        };
        bucket_prefix_contacts.insert(bucket_index, prefix_contact);

        let new_prefix = prefix_entry.destination();
        // register new bucket prefix (NodeIdSubnet) and remove old one if changed
        if let Some(old_prefix) = bucket_prefix_entries
            .insert(bucket_index, prefix_entry.destination().clone())
            .filter(|old_prefix| old_prefix != new_prefix)
        {
            tracing::debug!(
                target: "derive_fwd_table_entries",
                bucket_index,
                reason = "bucket_prefix_changed",
                kind = "NodeId",
                %old_prefix, %new_prefix,
                "remove entry",
            );
            context
                .runtime()
                .update_fwd_tables(NodeIdTableUpdate::Remove(old_prefix));
        }

        tracing::debug!(
            target: "derive_fwd_table_entries",
            bucket_index,
            kind = "NodeId",
            entry = %prefix_entry,
            "create entry",
        );
        context
            .runtime()
            .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(prefix_entry));

        Ok(())
    }

    fn derive_node_id_entry(
        &self,
        context: &C,
        contact: &Contact,
    ) -> Result<NodeIdEntry, DeriveFwdEntriesError> {
        let next_hop = *contact
            .path()
            .expect("contact is expected to have an active path")
            .first();
        let (next_hop, _) = context
            .uln_table()
            .get(&next_hop)
            .copied()
            .ok_or(DeriveFwdEntriesError::NeighborNotInULNTable(next_hop))?;

        // A confirmed underlay neighbor has a direct forwarding entry that must not be replaced.
        // A tentative one has no confirmed direct link, so it is reached via a longer path.
        let is_bidirectional_uln = matches!(
            context.uln_table().get(contact.id()),
            Some((_, ULNState::Bidirectional))
        );
        if is_bidirectional_uln && !contact.is_uln() {
            log::warn!(target: "derive_fwd_table_entries",
                "Contact {contact:?} is not a underlay neighbor but listed as bidirectional in ULNTable -> may overwrite previous route unintentionally!");
            return Err(DeriveFwdEntriesError::NonUNInULNTable(Box::new(
                contact.clone(),
            )));
        }

        if contact.is_uln() {
            Ok(NodeIdEntry::Forward(NodeIdForwardingEntry {
                destination: NodeIdSubnet::new(*contact.id()),
                next_hop,
            }))
        } else {
            let (out_path_id, second_path_id) = self.segment_path_ids(contact.path().unwrap());
            Ok(NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination: NodeIdSubnet::new(*contact.id()),
                next_hop,
                out_path_id,
                second_path_id,
            }))
        }
    }

    /// Returns if the forwarding tier is able to forward data packets to the contact.
    fn is_forwarding_established(&self, contact: &Contact) -> bool {
        contact
            .path()
            .is_some_and(|path| path.is_forwarding_established(self.config.segmentation))
    }

    /// Returns if the first segment of the active path to the contact can be used by this
    /// node: the contact needs no entry in the PathId forwarding table (underlay neighbors and
    /// precomputed paths in the vicinity) or the entry for this path was installed (see
    /// [Self::derive_local_entry]).
    ///
    /// Otherwise, packets encapsulated with the PathId of the first segment are dropped.
    fn has_local_entry(&self, contact: &Contact) -> bool {
        let Some(path) = contact.path() else {
            return false;
        };
        if contact.is_uln()
            || PathSegments::split(path, self.config.segmentation)
                .first()
                .len()
                < VICINITY_RADIUS
        {
            return true;
        }
        let (out_path_id, _) = self.segment_path_ids(path);
        matches!(
            &self.state,
            DeriveFwdTableEntriesState::Running { local_entries, .. }
                if local_entries.get(contact.id()).is_some_and(|entry| matches!(
                    entry,
                    PathIdEntry::Forward(PathIdForwardingEntry { out_path_id: local, .. })
                        if local == &out_path_id
                ))
        )
    }

    /// Returns the [PathId]s of the first and second [segment](PathSegments) of the path.
    fn segment_path_ids(&self, path: &Path) -> (PathId, Option<PathId>) {
        let segments = PathSegments::split(path, self.config.segmentation);
        (
            self.config.hasher.hash(segments.first()),
            segments
                .second()
                .map(|second| self.config.hasher.hash(second)),
        )
    }

    /// Derives the entry of the PathId forwarding table for the first segment of the path
    /// to the contact, which the given [NodeIdEntry] of the contact refers to.
    ///
    /// Underlay neighbors are reached without PathId. Entries of paths in the vicinity are
    /// precomputed (see [PrecomputePathIds](super::precompute_paths_and_path_ids::PrecomputePathIds))
    /// and need no reference of the contact.
    fn derive_local_entry(
        &self,
        context: &C,
        contact: &Contact,
        entry: &NodeIdEntry,
    ) -> Option<PathIdEntry> {
        let NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
            out_path_id,
            next_hop,
            ..
        }) = entry
        else {
            return None;
        };
        let first_segment = PathSegments::split(contact.path()?, self.config.segmentation).first();
        if first_segment.len() < VICINITY_RADIUS {
            return None;
        }
        let in_path_id = self
            .config
            .hasher
            .hash(std::iter::once(context.root_id()).chain(first_segment));

        Some(PathIdEntry::Forward(PathIdForwardingEntry {
            in_path_id,
            out_path_id: out_path_id.clone(),
            next_hop: *next_hop,
        }))
    }

    /// Sets the entry of the PathId forwarding table used by the contact
    /// and releases the one used before.
    fn set_local_entry(&mut self, context: &C, node_id: &NodeId, entry: Option<PathIdEntry>) {
        let DeriveFwdTableEntriesState::Running { local_entries, .. } = &mut self.state else {
            panic!("set_local_entry called without running DeriveFwdTableEntries");
        };
        if local_entries.get(node_id) == entry.as_ref() {
            return;
        }

        let old_entry = match entry {
            Some(entry) => {
                // add the new one first, in case both refer to the same PathId
                let update = context.fwd_state_mut().add_local(entry.clone());
                if let Some(update) = update {
                    context.runtime().update_fwd_tables(update);
                }
                local_entries.insert(*node_id, entry)
            }
            None => local_entries.remove(node_id),
        };
        if let Some(old_entry) = old_entry {
            let update = context
                .fwd_state_mut()
                .release_local(old_entry.in_path_id());
            if let Some(update) = update {
                context.runtime().update_fwd_tables(update);
            }
        }
    }

    /// Forgets the entries via the lost underlay neighbor.
    ///
    /// They are removed from the [PathIdFwdState](crate::domain::PathIdFwdState) altogether,
    /// so they must not be released later on.
    fn forget_local_entries_via(&mut self, ulnid: &UnderlayNeighborId) {
        if let DeriveFwdTableEntriesState::Running { local_entries, .. } = &mut self.state {
            local_entries.retain(|_, entry| {
                !matches!(entry, PathIdEntry::Forward(PathIdForwardingEntry { next_hop, .. }) if next_hop == ulnid)
            });
        }
    }

    /// Derives one prefix entry of the bucket using proximity neighbor selection.
    ///
    /// If the bucket is empty [None] is returned
    fn derive_bucket_prefix_entry(
        &self,
        context: &C,
        bucket_index: usize,
    ) -> Result<Option<(NodeIdEntry, NodeId)>, DeriveFwdEntriesError> {
        let rt = context.routing_table();
        let bucket: &Bucket<_> = rt.bucket_by_index(bucket_index);
        let iter = bucket.iter();
        let prefix_len = rt.get_bucket_prefix_length(bucket_index);

        // Proximity Neighbor Selection:
        // Select contact with shortest path in bucket as prefix entry
        let Some(closest) = iter
            .filter(|c| {
                c.is_valid() && self.is_forwarding_established(c) && self.has_local_entry(c)
            })
            .min_by_key(|c| {
                c.path()
                    .expect("valid contact should have an active path")
                    .size()
            })
        else {
            tracing::trace!(
                target: "derive_fwd_table_entries",
                %bucket_index,
                ?bucket,
                reason = "empty_bucket",
                "no prefix entry derived",
            );

            return Ok(None);
        };
        assert_ne!(closest.id(), context.root_id(), "root in not-lowest bucket");

        let subnet = NodeIdSubnet::try_new(closest.id().prefix(prefix_len), prefix_len)
            .expect("should be valid prefix length");

        let next_hop = *closest.path().unwrap().first();
        let (next_hop, _) = context
            .uln_table()
            .get(&next_hop)
            .copied()
            .ok_or(DeriveFwdEntriesError::NeighborNotInULNTable(next_hop))?;

        let entry = if closest.is_uln() {
            NodeIdEntry::Forward(NodeIdForwardingEntry {
                destination: subnet,
                next_hop,
            })
        } else {
            let (out_path_id, second_path_id) = self.segment_path_ids(closest.path().unwrap());
            NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination: subnet,
                next_hop,
                out_path_id,
                second_path_id,
            })
        };

        tracing::trace!(
            target: "derive_fwd_table_entries",
            %bucket_index,
            ?bucket,
            %entry,
            "derived prefix entry"
        );

        Ok(Some((entry, *closest.id())))
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = DeriveFwdEntriesError;
    type Value = ();

    #[instrument(
        target = "derive_fwd_table_entries",
        "derive_fwd_table_entries",
        skip(self, context, event),
        fields(
            state = ?self.state,
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        let contact_event = match event {
            UseCaseEvent::Contact(contact_event) => contact_event,
            UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::UnderlayNeighborDown(ulnid)) => {
                self.forget_local_entries_via(&ulnid);
                return Ok(());
            }
            _ => return Ok(()),
        };
        match *contact_event {
            // The entry of a contact whose path requires a setup is created as soon as the
            // setup is confirmed, which is signaled by an update of the contact.
            ContactEvent::New(contact) => {
                let _span = tracing::debug_span!(
                    target: "derive_fwd_table_entries",
                    "create_entry",
                    reason = "contact_new",
                    node = %contact.id(),
                    kind = "NodeId",
                )
                .entered();
                self.create_node_id_entry(context, contact)?;
            }
            ContactEvent::Removed(contact) => {
                let _span = tracing::debug_span!(
                    target: "derive_fwd_table_entries",
                    "remove_entry",
                    reason = "contact_removed",
                    node = %contact.id(),
                    kind = "NodeId",
                )
                .entered();
                self.remove_node_id_entry(context, contact.id())?;
            }
            ContactEvent::Updated { new, old } => {
                let updated_span = tracing::debug_span!(
                    target: "derive_fwd_table_entries",
                    "process_contact_update",
                    kind = field::Empty,
                    ?new, ?old,
                )
                .entered();

                match (new.state(), old.state()) {
                    (ContactState::Invalid(_), ContactState::Valid)
                    | (ContactState::Rediscovering(_), ContactState::Valid)
                        if new.id() == old.id() =>
                    {
                        updated_span.record("kind", "contact_invalidation");

                        let _span = tracing::debug_span!(
                            target: "derive_fwd_table_entries",
                            "remove_entry",
                            reason = "contact_invalidated",
                            node = %new.id(),
                            kind = "NodeId",
                        )
                        .entered();
                        self.remove_node_id_entry(context, new.id())?;
                    }
                    (ContactState::Invalid(_), ContactState::Valid) => {
                        updated_span.record("kind", "contact_invalidation_by_displacement");
                        tracing::warn!(
                            target: "derive_fwd_table_entries",
                            new_destination = %new.id(),
                            old_destination = %old.id(),
                            "valid contact replaced by invalid but with different destination",
                        );

                        let _span = tracing::debug_span!(
                            target: "derive_fwd_table_entries",
                            "remove_entry",
                            reason = "contact_invalid_displacer",
                            node = %new.id(),
                            kind = "NodeId",
                        )
                        .entered();
                        self.remove_node_id_entry(context, new.id())?;

                        let _span = tracing::debug_span!(
                            target: "derive_fwd_table_entries",
                            "remove_entry",
                            reason = "contact_displaced",
                            displacer = %new.id(),
                            node = %old.id(),
                            kind = "NodeId",
                        )
                        .entered();
                        self.remove_node_id_entry(context, old.id())?;
                    }
                    (ContactState::Valid, ContactState::Invalid(_))
                    | (ContactState::Valid, ContactState::Rediscovering(_)) => {
                        updated_span.record("kind", "contact_validation");

                        let _span = tracing::debug_span!(
                            target: "derive_fwd_table_entries",
                            "create_entry",
                            reason = "contact_validated",
                            node = %new.id(),
                            kind = "NodeId",
                        )
                        .entered();
                        self.create_node_id_entry(context, *new)?;
                    }
                    (ContactState::Valid, ContactState::Valid) if new.path() != old.path() => {
                        if new.id() == old.id() {
                            // just a simple path change to the same destination
                            updated_span.record("kind", "path_change");

                            let _span = tracing::debug_span!(
                                target: "derive_fwd_table_entries",
                                "update_entry",
                                reason = "path_changed",
                                node = %new.id(),
                                kind = "NodeId",
                            )
                            .entered();
                            self.update_node_id_entry(context, *new)?;
                        } else {
                            // Contact got substituted for another destination
                            // probably due to proximity neighbor selection
                            updated_span.record("kind", "contact_displacement");

                            let _span = tracing::debug_span!(
                                target: "derive_fwd_table_entries",
                                "remove_entry",
                                reason = "contact_displaced",
                                node = %old.id(),
                                kind = "NodeId",
                            )
                            .entered();
                            self.remove_node_id_entry(context, old.id())?;

                            let _span = tracing::debug_span!(
                                target: "derive_fwd_table_entries",
                                "create_entry",
                                reason = "displaced_contact",
                                node = %new.id(),
                                kind = "NodeId",
                            )
                            .entered();
                            self.create_node_id_entry(context, *new)?;
                        }
                    }
                    _ => {}
                }
            }
            ContactEvent::BucketUpdated(bucket) => {
                let _span = tracing::debug_span!(
                    target: "derive_fwd_table_entries",
                    "update_bucket_entries",
                    bucket_index = bucket,
                    reason = "bucket_changed",
                    kind = "NodeId",
                )
                .entered();
                self.update_bucket(context, bucket)?;
            }
            ContactEvent::NewBucket(bucket) => {
                let _span = tracing::debug_span!(
                    target: "derive_fwd_table_entries",
                    "update_bucket_entries",
                    bucket_index = bucket,
                    reason = "new_bucket",
                    kind = "NodeId",
                )
                .entered();
                self.update_bucket(context, bucket)?;
            }
            ContactEvent::Proposed(_) => { // not relevant for this use case
            }
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type State = DeriveFwdTableEntriesState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let in_path = Path::from(*context.root_id());
        let in_path_id = self.config.hasher.hash(&in_path);
        let entry = PathIdDecapsulationEntry {
            in_path_id,
            next_hop: DecapsulationDestination::Local,
        };

        self.state = DeriveFwdTableEntriesState::Running {
            bucket_prefix_entries: HashMap::with_capacity(2),
            bucket_prefix_contacts: HashMap::with_capacity(2),
            local_entries: HashMap::new(),
        };

        // packets destined to the own node, never removed
        let update = context
            .fwd_state_mut()
            .add_precalc(PathIdEntry::Decapsulate(entry));
        if let Some(update) = update {
            context.runtime().update_fwd_tables(update);
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug, Display)]
pub enum DeriveFwdEntriesError {
    #[display("Neighbor listed in contacts path not in ULNTable: {_0}")]
    NeighborNotInULNTable(NodeId),
    #[display("Contact is not a underlay neighbor but listed as bidirectional in ULNTable: {_0}")]
    NonUNInULNTable(Box<Contact>),
}
impl Error for DeriveFwdEntriesError {}

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
            PathIdTableUpdate,
            SafeStateSeqNr,
            protocol_event::forwarding::ForwardingTablesUpdate,
            routing_table::SingleBucketRT,
        },
        runtime::testing::TestingUseCaseRuntime,
    };

    mod vicinity {
        //! Tests that DeriveFwdEntries will not change any paths inside the vicinity

        use super::*;
        use crate::domain::{
            ConnectionId,
            InterfaceId,
        };

        #[test]
        fn vicinity_contact_added() {
            let _clock = crate::tests::TestClock::new();
            crate::tests::init();

            let runtime = TestingUseCaseRuntime::default();

            let ulnid = {
                let interface_id = InterfaceId::try_from(1).unwrap();
                let conn_id = ConnectionId::from(0);

                UnderlayNeighborId {
                    interface_id,
                    connection_id: conn_id,
                }
            };
            let root_id = NodeId::with_lsb(1);
            let neighbor_id = NodeId::with_lsb(2);
            let vicinity_contact_id = NodeId::with_lsb(3);

            let neighbor = Contact::new(
                Path::from(neighbor_id),
                SafeStateSeqNr::try_from(1).unwrap(),
            );
            let vicinity_contact = Contact::new(
                Path::from([neighbor_id, vicinity_contact_id]),
                SafeStateSeqNr::try_from(4).unwrap(),
            );

            let mut routing_table = SingleBucketRT::<20>::new(root_id);
            assert!(routing_table.insert(neighbor.clone()).is_ok());

            let mut uln_table = Box::new(HashMap::new());
            uln_table.insert(neighbor_id, (ulnid, ULNState::Bidirectional));

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Shake256,
                ..Default::default()
            };
            let mut use_case = DeriveFwdTableEntries::new(config);
            assert!(use_case.start(&sync_context).is_ok(), "starting failed");
            let _ = sync_context.runtime().output();

            let event =
                UseCaseEvent::Contact(Box::new(ContactEvent::New(vicinity_contact.clone())));
            let handle_result = use_case.handle_event(&sync_context, event);
            assert!(
                handle_result.is_ok(),
                "Error handling event: {handle_result:?}"
            );

            let output: Vec<_> = sync_context.runtime().output().collect();

            let node_id_update = output
                .iter()
                .find_map(|o| {
                    if let Output::UpdateForwardingTables(
                        ForwardingTablesUpdate::NodeIdTableUpdate(node_id_update),
                    ) = o
                    {
                        Some(node_id_update)
                    } else {
                        None
                    }
                })
                .expect("No node id table update generated for added contact");
            let NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Encapsulate(encap_entry)) =
                node_id_update
            else {
                panic!("unexpected kind of node id table update: {node_id_update:?}");
            };

            assert_eq!(encap_entry.destination.node_id(), vicinity_contact.id());
            assert!(
                encap_entry.destination.prefix_length() == 0
                    || encap_entry.destination.prefix_length() == NodeId::BITS,
            );
            assert_eq!(encap_entry.next_hop, ulnid);
            assert_eq!(
                encap_entry.out_path_id,
                //Hasher::Shake256.hash(vicinity_contact.path().into_iter().skip(1))
                Hasher::Shake256.hash(vicinity_contact.path().unwrap().into_iter())
            );

            let path_id_update = output.iter().find_map(|o| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    path_id_update,
                )) = o
                {
                    Some(path_id_update)
                } else {
                    None
                }
            });
            if let Some(path_id_update) = path_id_update {
                panic!(
                    "No path id updates for already setup paths inside the vicinity: {path_id_update}"
                );
            }

            let fwd_updates = output
                .iter()
                .filter_map(|o| {
                    if let Output::UpdateForwardingTables(fwd_update) = o {
                        Some(fwd_update)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(
                fwd_updates.len(),
                2, // +1 for prefix update
                "Additional forwarding updates present: {fwd_updates:#?}"
            );
        }

        #[test]
        fn uln_contact_added() {
            let _clock = crate::tests::TestClock::new();
            crate::tests::init();

            let runtime = TestingUseCaseRuntime::default();

            let ulnid = {
                let interface_id = InterfaceId::try_from(1).unwrap();
                let conn_id = ConnectionId::from(0);

                UnderlayNeighborId {
                    interface_id,
                    connection_id: conn_id,
                }
            };
            let root_id = NodeId::with_lsb(1);
            let neighbor_id = NodeId::with_lsb(2);

            let neighbor = Contact::new(
                Path::from(neighbor_id),
                SafeStateSeqNr::try_from(1).unwrap(),
            );

            let mut routing_table = SingleBucketRT::<20>::new(root_id);
            assert!(routing_table.insert(neighbor.clone()).is_ok());

            let mut uln_table = Box::new(HashMap::new());
            uln_table.insert(neighbor_id, (ulnid, ULNState::Bidirectional));

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Shake256,
                ..Default::default()
            };
            let mut use_case = DeriveFwdTableEntries::new(config);
            assert!(use_case.start(&sync_context).is_ok(), "starting failed");
            let _ = sync_context.runtime().output();

            let event = UseCaseEvent::Contact(Box::new(ContactEvent::New(neighbor.clone())));
            let handle_result = use_case.handle_event(&sync_context, event);
            assert!(
                handle_result.is_ok(),
                "Error handling event: {handle_result:?}"
            );

            let output: Vec<_> = sync_context.runtime().output().collect();

            let node_id_update = output
                .iter()
                .find_map(|o| {
                    if let Output::UpdateForwardingTables(
                        ForwardingTablesUpdate::NodeIdTableUpdate(node_id_update),
                    ) = o
                    {
                        Some(node_id_update)
                    } else {
                        None
                    }
                })
                .expect("No node id table update generated for added contact");
            let NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Forward(fwd_entry)) = node_id_update
            else {
                panic!("unexpected kind of node id table update: {node_id_update:?}");
            };

            assert_eq!(fwd_entry.destination.node_id(), neighbor.id());
            assert!(
                fwd_entry.destination.prefix_length() == 0
                    || fwd_entry.destination.prefix_length() == NodeId::BITS,
            );
            assert_eq!(fwd_entry.next_hop, ulnid);

            let path_id_update = output.iter().find_map(|o| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    path_id_update,
                )) = o
                {
                    Some(path_id_update)
                } else {
                    None
                }
            });
            assert!(
                path_id_update.is_none(),
                "No path id updates for neighbors: {}",
                path_id_update.unwrap()
            );

            let fwd_updates = output
                .iter()
                .filter_map(|o| {
                    if let Output::UpdateForwardingTables(fwd_update) = o {
                        Some(fwd_update)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(
                fwd_updates.len(),
                2, // +1 for prefix update
                "Additional forwarding updates present: {fwd_updates:#?}"
            );
        }

        #[test]
        fn vicinity_contact_removed() {
            let _clock = crate::tests::TestClock::new();
            crate::tests::init();

            let runtime = TestingUseCaseRuntime::default();

            let ulnid = {
                let interface_id = InterfaceId::try_from(1).unwrap();
                let conn_id = ConnectionId::from(0);

                UnderlayNeighborId {
                    interface_id,
                    connection_id: conn_id,
                }
            };
            let root_id = NodeId::with_lsb(1);
            let neighbor_id = NodeId::with_lsb(2);
            let vicinity_contact_id = NodeId::with_lsb(3);

            let neighbor = Contact::new(
                Path::from(neighbor_id),
                SafeStateSeqNr::try_from(1).unwrap(),
            );
            let vicinity_contact = Contact::new(
                Path::from([neighbor_id, vicinity_contact_id]),
                SafeStateSeqNr::try_from(4).unwrap(),
            );

            let mut routing_table = SingleBucketRT::<20>::new(root_id);
            assert!(routing_table.insert(neighbor.clone()).is_ok());

            let mut uln_table = Box::new(HashMap::new());
            uln_table.insert(neighbor_id, (ulnid, ULNState::Bidirectional));

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Shake256,
                ..Default::default()
            };
            let mut use_case = DeriveFwdTableEntries::new(config);
            assert!(use_case.start(&sync_context).is_ok(), "starting failed");
            let _ = sync_context.runtime().output();

            let event =
                UseCaseEvent::Contact(Box::new(ContactEvent::Removed(vicinity_contact.clone())));
            let handle_result = use_case.handle_event(&sync_context, event);
            assert!(
                handle_result.is_ok(),
                "Error handling event: {handle_result:?}"
            );

            let output: Vec<_> = sync_context.runtime().output().collect();

            let node_id_update = output
                .iter()
                .find_map(|o| {
                    if let Output::UpdateForwardingTables(
                        ForwardingTablesUpdate::NodeIdTableUpdate(node_id_update),
                    ) = o
                    {
                        Some(node_id_update)
                    } else {
                        None
                    }
                })
                .expect("No node id table update generated for removed  contact");
            let NodeIdTableUpdate::Remove(removed_subnet) = node_id_update else {
                panic!("unexpected kind of node id table update: {node_id_update:?}");
            };
            assert_eq!(removed_subnet.node_id(), vicinity_contact.id(),);

            let path_id_update = output.iter().find_map(|o| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    path_id_update,
                )) = o
                {
                    Some(path_id_update)
                } else {
                    None
                }
            });

            assert!(
                path_id_update.is_none(),
                "No path id updates for already setup paths inside the vicinity: {}",
                path_id_update.unwrap()
            );

            let fwd_updates = output
                .iter()
                .filter_map(|o| {
                    if let Output::UpdateForwardingTables(fwd_update) = o {
                        Some(fwd_update)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(
                fwd_updates.len(),
                2, // +1 for prefix update
                "Additional forwarding updates present: {fwd_updates:#?}"
            );
        }

        #[test]
        /// Updates a contact inside the vicinity to a contact outside the vicinity.
        fn vicinity_contact_updated() {
            let _clock = crate::tests::TestClock::new();
            crate::tests::init();

            let runtime = TestingUseCaseRuntime::default();

            let ulnid = {
                let interface_id = InterfaceId::try_from(1).unwrap();
                let conn_id = ConnectionId::from(0);

                UnderlayNeighborId {
                    interface_id,
                    connection_id: conn_id,
                }
            };
            let root_id = NodeId::with_lsb(1);
            let neighbor_id = NodeId::with_lsb(2);
            let vicinity_contact_id = NodeId::with_lsb(3);

            let neighbor = Contact::new(
                Path::from(neighbor_id),
                SafeStateSeqNr::try_from(1).unwrap(),
            );
            let vicinity_contact = Contact::new(
                Path::from([neighbor_id, vicinity_contact_id]),
                SafeStateSeqNr::try_from(4).unwrap(),
            );

            let mut routing_table = SingleBucketRT::<20>::new(root_id);
            assert!(routing_table.insert(neighbor.clone()).is_ok());

            let mut uln_table = Box::new(HashMap::new());
            uln_table.insert(neighbor_id, (ulnid, ULNState::Bidirectional));

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Shake256,
                ..Default::default()
            };
            let mut use_case = DeriveFwdTableEntries::new(config);
            assert!(use_case.start(&sync_context).is_ok(), "starting failed");
            let _ = sync_context.runtime().output();

            let new_contact = Contact::new(
                Path::from([
                    neighbor_id,
                    NodeId::with_lsb(4),
                    NodeId::with_lsb(5),
                    vicinity_contact_id,
                ]),
                SafeStateSeqNr::try_from(15).unwrap(),
            );

            let mut new_in_path = Path::from(root_id);
            new_in_path.extend(new_contact.path().unwrap().clone());
            // a path of four hops is split into two segments of two hops each,
            // the PathId of the second one includes the last node of the first one
            let new_out_path_id = Hasher::Shake256.hash(&[neighbor_id, NodeId::with_lsb(4)]);
            let new_second_path_id = Hasher::Shake256.hash(&[
                NodeId::with_lsb(4),
                NodeId::with_lsb(5),
                vicinity_contact_id,
            ]);

            let event = UseCaseEvent::Contact(Box::new(ContactEvent::Updated {
                new: Box::new(new_contact.clone()),
                old: Box::new(vicinity_contact.clone()),
            }));
            let handle_result = use_case.handle_event(&sync_context, event);
            assert!(
                handle_result.is_ok(),
                "Error handling event: {handle_result:?}"
            );

            let output: Vec<_> = sync_context.runtime().output().collect();

            // Node id entry is updated
            let node_id_update = output
                .iter()
                .find_map(|o| {
                    if let Output::UpdateForwardingTables(
                        ForwardingTablesUpdate::NodeIdTableUpdate(node_id_update),
                    ) = o
                    {
                        Some(node_id_update)
                    } else {
                        None
                    }
                })
                .expect("No node id table update generated for updated contact");
            let NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Encapsulate(node_id_entry)) =
                node_id_update
            else {
                panic!("unexpected kind of node id table update: {node_id_update:?}");
            };
            assert_eq!(node_id_entry.destination.node_id(), new_contact.id());
            assert!(
                node_id_entry.destination.prefix_length() == 0
                    || node_id_entry.destination.prefix_length() == NodeId::BITS,
            );
            assert_eq!(node_id_entry.next_hop, ulnid);
            assert_eq!(node_id_entry.out_path_id, new_out_path_id,);
            assert_eq!(node_id_entry.second_path_id, Some(new_second_path_id));

            // path id entries inside the precomputed vicinity are not allowed to be removed
            let removed_pid = output.iter().find_map(|o| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    PathIdTableUpdate::Remove(removed_pid),
                )) = o
                {
                    Some(removed_pid)
                } else {
                    None
                }
            });
            assert!(
                removed_pid.is_none(),
                "No removal of PathIdTable entries for paths inside the vicinity: {}",
                removed_pid.unwrap()
            );
            let path_id_entry = output.iter().find_map(|o| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    path_id_entry,
                )) = o
                {
                    Some(path_id_entry)
                } else {
                    None
                }
            });
            assert!(
                path_id_entry.is_none(),
                "No creation of additional PathIDTable entries necessary: {}",
                path_id_entry.unwrap()
            );

            // check for additional updates
            let fwd_updates = output
                .iter()
                .filter_map(|o| {
                    if let Output::UpdateForwardingTables(fwd_update) = o {
                        Some(fwd_update)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(
                fwd_updates.len(),
                2, // +1 for prefix update
                "Additional forwarding updates present: {fwd_updates:#?}"
            );
        }
    }

    mod uln_state {
        //! Contacts of nodes that are also listed in the ULNTable

        use super::*;
        use crate::domain::{
            ConnectionId,
            InterfaceId,
        };

        fn ulnid() -> UnderlayNeighborId {
            UnderlayNeighborId {
                interface_id: InterfaceId::try_from(1).unwrap(),
                connection_id: ConnectionId::from(0),
            }
        }

        /// Derives the NodeId entry of a contact <neighbor, other> while `other` is in the
        /// ULNTable with the given state.
        fn derive(other_state: ULNState) -> Result<NodeIdEntry, DeriveFwdEntriesError> {
            crate::tests::init();
            let root_id = NodeId::with_lsb(1);
            let neighbor_id = NodeId::with_lsb(2);
            let other_id = NodeId::with_lsb(3);

            let mut uln_table = Box::new(HashMap::new());
            uln_table.insert(neighbor_id, (ulnid(), ULNState::Bidirectional));
            uln_table.insert(other_id, (ulnid(), other_state));

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table: SingleBucketRT::<20>::new(root_id),
                uln_table,
                insertion_strategy: (),
                runtime: TestingUseCaseRuntime::default(),
                vicinity_graph: (),
            });
            let use_case = DeriveFwdTableEntries::new(DeriveFwdTableEntriesConfig {
                hasher: Hasher::Shake256,
                ..Default::default()
            });

            let contact = Contact::new(
                Path::from([neighbor_id, other_id]),
                SafeStateSeqNr::try_from(1).unwrap(),
            );
            use_case.derive_node_id_entry(&sync_context, &contact)
        }

        #[test]
        fn tentative_neighbor_reached_via_longer_path() {
            let _clock = crate::tests::TestClock::new();
            // the direct link is not confirmed (e.g. asymmetric), so the longer path is used
            let entry = derive(ULNState::Tentative).expect("entry for tentative neighbor");
            assert!(matches!(entry, NodeIdEntry::Encapsulate(_)));
        }

        #[test]
        fn bidirectional_neighbor_via_longer_path_is_rejected() {
            let _clock = crate::tests::TestClock::new();
            assert!(matches!(
                derive(ULNState::Bidirectional),
                Err(DeriveFwdEntriesError::NonUNInULNTable(_))
            ));
        }
    }

    mod segments {
        //! Contacts whose paths are split into two segments

        use super::*;
        use crate::domain::{
            ConnectionId,
            InterfaceId,
            Timestamp,
        };

        type Ctx = SyncContext<
            SingleBucketRT<20>,
            TestingUseCaseRuntime,
            (),
            Box<HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
            (),
        >;

        fn ulnid() -> UnderlayNeighborId {
            UnderlayNeighborId {
                interface_id: InterfaceId::try_from(1).unwrap(),
                connection_id: ConnectionId::from(0),
            }
        }

        fn node(index: u8) -> NodeId {
            NodeId::with_lsb(index)
        }

        /// Contact with a path of the given number of hops via the nodes 2, 3, ...
        fn contact(hops: u8) -> Contact {
            let path: Vec<_> = (2..2 + hops).map(node).collect();
            Contact::new(
                Path::try_from(path).unwrap(),
                SafeStateSeqNr::try_from(1).unwrap(),
            )
        }

        fn hash(nodes: std::ops::RangeInclusive<u8>) -> PathId {
            Hasher::Shake256.hash(&nodes.map(node).collect::<Vec<_>>())
        }

        fn setup() -> (DeriveFwdTableEntries<Ctx, 20>, Ctx) {
            crate::tests::init();
            let root_id = node(1);
            let mut uln_table = Box::new(HashMap::new());
            uln_table.insert(node(2), (ulnid(), ULNState::Bidirectional));

            let ctx = SyncContext::new(ContextConfig {
                root_id,
                routing_table: SingleBucketRT::<20>::new(root_id),
                uln_table,
                insertion_strategy: (),
                runtime: TestingUseCaseRuntime::default(),
                vicinity_graph: (),
            });
            let mut use_case = DeriveFwdTableEntries::new(DeriveFwdTableEntriesConfig {
                hasher: Hasher::Shake256,
                ..Default::default()
            });
            use_case.start(&ctx).unwrap();
            let _ = ctx.runtime().output();
            (use_case, ctx)
        }

        fn handle(use_case: &mut DeriveFwdTableEntries<Ctx, 20>, ctx: &Ctx, event: ContactEvent) {
            use_case
                .handle_event(ctx, UseCaseEvent::Contact(Box::new(event)))
                .unwrap();
        }

        /// Encapsulation entries of the contact (not of its bucket, whose prefix is zero) created,
        /// PathId entries installed and incoming PathIds removed since the last call.
        fn output(ctx: &Ctx) -> (Vec<NodeIdEncapsulationEntry>, Vec<PathIdEntry>, Vec<PathId>) {
            let (mut encaps, mut installed, mut removed) = (Vec::new(), Vec::new(), Vec::new());
            for output in ctx.runtime().output() {
                let Output::UpdateForwardingTables(update) = output else {
                    panic!("unexpected output {output:?}");
                };
                match update {
                    ForwardingTablesUpdate::NodeIdTableUpdate(
                        NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Encapsulate(entry)),
                    ) if entry.destination.node_id() != &node(0) => encaps.push(entry),
                    ForwardingTablesUpdate::PathIdTableUpdate(
                        PathIdTableUpdate::CreateOrUpdate(entry),
                    ) => installed.push(entry),
                    ForwardingTablesUpdate::PathIdTableUpdate(PathIdTableUpdate::Remove(id)) => {
                        removed.push(id)
                    }
                    _ => {}
                }
            }
            (encaps, installed, removed)
        }

        #[test]
        fn three_hop_path_uses_single_path_id_and_local_entry() {
            let _clock = crate::tests::TestClock::new();
            let (mut use_case, ctx) = setup();
            let contact = contact(3); // 2 3 4

            handle(&mut use_case, &ctx, ContactEvent::New(contact.clone()));
            let (encaps, installed, removed) = output(&ctx);
            assert_eq!(encaps.len(), 1);
            assert_eq!(encaps[0].out_path_id, hash(2..=4));
            assert_eq!(encaps[0].second_path_id, None);
            // H(1|2|3|4) -> H(2|3|4) is not precomputed
            assert_eq!(
                installed,
                [PathIdEntry::Forward(PathIdForwardingEntry {
                    in_path_id: hash(1..=4),
                    out_path_id: hash(2..=4),
                    next_hop: ulnid(),
                })]
            );
            assert!(removed.is_empty());

            handle(&mut use_case, &ctx, ContactEvent::Removed(contact));
            assert_eq!(output(&ctx).2, [hash(1..=4)]);
        }

        #[test]
        fn five_hop_path_is_split_without_setup() {
            let _clock = crate::tests::TestClock::new();
            let (mut use_case, ctx) = setup();

            handle(&mut use_case, &ctx, ContactEvent::New(contact(5))); // 2 3 4 | 5 6
            let (encaps, ..) = output(&ctx);
            assert_eq!(encaps.len(), 1);
            assert_eq!(encaps[0].out_path_id, hash(2..=4));
            assert_eq!(encaps[0].second_path_id, Some(hash(4..=6)));
        }

        #[test]
        fn six_hop_path_is_used_once_established() {
            let _clock = crate::tests::TestClock::new();
            let (mut use_case, ctx) = setup();
            let contact = contact(6); // 2 3 4 | 5 6 7

            handle(&mut use_case, &ctx, ContactEvent::New(contact.clone()));
            let (encaps, installed, _) = output(&ctx);
            assert!(encaps.is_empty(), "path setup not confirmed yet");
            assert!(installed.is_empty());

            // PathSetupRsp or ProbeRsp
            let mut established = contact.clone();
            established
                .path_mut()
                .unwrap()
                .update_last_path_refresh(Timestamp::now());
            handle(
                &mut use_case,
                &ctx,
                ContactEvent::Updated {
                    new: Box::new(established),
                    old: Box::new(contact),
                },
            );
            let (encaps, installed, _) = output(&ctx);
            assert_eq!(encaps.len(), 1);
            assert_eq!(encaps[0].out_path_id, hash(2..=4));
            assert_eq!(encaps[0].second_path_id, Some(hash(4..=7)));
            assert_eq!(installed.len(), 1);
            assert_eq!(installed[0].in_path_id(), &hash(1..=4));
        }

        #[test]
        fn bucket_entry_only_uses_contacts_with_installed_local_entry() {
            let _clock = crate::tests::TestClock::new();
            let (mut use_case, ctx) = setup();
            let established = |hops| {
                let mut contact = contact(hops);
                contact
                    .path_mut()
                    .unwrap()
                    .update_last_path_refresh(Timestamp::now());
                contact
            };
            // the establishment of x was not notified, so its local entry is missing
            let x = established(6); // 2 3 4 | 5 6 7
            let y = established(7); // 2 3 4 5 | 5 6 7 8
            ctx.routing_table_mut().add(x).unwrap();
            ctx.routing_table_mut().add(y.clone()).unwrap();

            handle(&mut use_case, &ctx, ContactEvent::New(y));

            let bucket_entries: Vec<_> = ctx
                .runtime()
                .output()
                .filter_map(|output| match output {
                    Output::UpdateForwardingTables(ForwardingTablesUpdate::NodeIdTableUpdate(
                        NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Encapsulate(entry)),
                    )) if entry.destination.node_id() == &node(0) => Some(entry),
                    _ => None,
                })
                .collect();
            assert_eq!(bucket_entries.len(), 1, "{bucket_entries:?}");
            // the shorter path of x can't be used by this node yet
            assert_eq!(bucket_entries[0].out_path_id, hash(2..=5));
            assert_eq!(bucket_entries[0].second_path_id, Some(hash(5..=8)));
        }

        /// Bucket prefix entries (whose prefix is zero) created since the last call
        fn bucket_entries(ctx: &Ctx) -> Vec<NodeIdEncapsulationEntry> {
            ctx.runtime()
                .output()
                .filter_map(|output| match output {
                    Output::UpdateForwardingTables(ForwardingTablesUpdate::NodeIdTableUpdate(
                        NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Encapsulate(entry)),
                    )) if entry.destination.node_id() == &node(0) => Some(entry),
                    _ => None,
                })
                .collect()
        }

        #[test]
        fn bucket_entry_of_invalidated_contact_is_derived_anew() {
            let _clock = crate::tests::TestClock::new();
            let (mut use_case, ctx) = setup();
            let near = contact(2); // 2 3
            let far = contact(3); // 2 3 4
            ctx.routing_table_mut().add(near.clone()).unwrap();
            ctx.routing_table_mut().add(far.clone()).unwrap();
            handle(&mut use_case, &ctx, ContactEvent::New(near.clone()));
            handle(&mut use_case, &ctx, ContactEvent::New(far));
            assert_eq!(
                bucket_entries(&ctx).last().unwrap().out_path_id,
                hash(2..=3),
                "the shorter path is used"
            );

            // the contact used by the bucket entry becomes unreachable
            let mut invalid = near.clone();
            invalid.set_invalid(crate::domain::NotViaStateList::default());
            *ctx.routing_table_mut().contact_mut(near.id()).unwrap() = invalid.clone();
            handle(
                &mut use_case,
                &ctx,
                ContactEvent::Updated {
                    new: Box::new(invalid),
                    old: Box::new(near),
                },
            );

            let entries = bucket_entries(&ctx);
            assert_eq!(entries.len(), 1, "bucket entry derived anew: {entries:?}");
            assert_eq!(entries[0].out_path_id, hash(2..=4));
        }

        #[test]
        fn contacts_sharing_first_segment_share_local_entry() {
            let _clock = crate::tests::TestClock::new();
            let (mut use_case, ctx) = setup();
            let near = contact(3); // 2 3 4
            let far = contact(5); // 2 3 4 | 5 6

            handle(&mut use_case, &ctx, ContactEvent::New(near.clone()));
            handle(&mut use_case, &ctx, ContactEvent::New(far.clone()));
            let (_, installed, _) = output(&ctx);
            assert_eq!(installed.len(), 1, "installed once");

            handle(&mut use_case, &ctx, ContactEvent::Removed(near));
            assert!(output(&ctx).2.is_empty(), "still used by the other contact");
            handle(&mut use_case, &ctx, ContactEvent::Removed(far));
            assert_eq!(output(&ctx).2, [hash(1..=4)]);
        }

        #[test]
        fn entries_via_lost_neighbor_are_not_released() {
            let _clock = crate::tests::TestClock::new();
            let (mut use_case, ctx) = setup();
            let contact = contact(3);
            handle(&mut use_case, &ctx, ContactEvent::New(contact.clone()));
            let _ = output(&ctx);

            // the entries via the neighbor are removed from the shared state altogether
            assert_eq!(ctx.fwd_state_mut().invalidate_via(&ulnid()).len(), 1);
            use_case
                .handle_event(
                    &ctx,
                    UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::UnderlayNeighborDown(
                        ulnid(),
                    )),
                )
                .unwrap();

            handle(&mut use_case, &ctx, ContactEvent::Removed(contact));
            let (_, installed, removed) = output(&ctx);
            assert!(installed.is_empty() && removed.is_empty());
        }
    }
}
