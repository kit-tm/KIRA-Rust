use derive_more::derive::Display;
use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::ops::Deref;
use tracing::{Level, field, instrument};

use crate::domain::protocol_event::forwarding::{
    DecapsulationDestination, NodeIdEncapsulationEntry, NodeIdEntry, NodeIdForwardingEntry,
    NodeIdTableUpdate, PathIdDecapsulationEntry, PathIdEntry, PathIdTableUpdate,
};
use crate::domain::{
    Contact, ContactState, NodeId, NodeIdSubnet, Path, RoutingTable, UnderlayNeighborId,
    hasher::Hasher,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    ContactEvent, EventHandler, UseCase, UseCaseContext, UseCaseEvent, UseCaseState,
};

/// Configuration for [DeriveFwdTableEntries] use case.
#[derive(Debug, Default)]
pub struct DeriveFwdTableEntriesConfig {
    /// Hashing algorithm to use.
    pub hasher: Hasher,
}

#[derive(Debug, Default)]
pub enum DeriveFwdTableEntriesState {
    #[default]
    /// [DeriveFwdTableEntries] is initialized.
    Initialized,
    Running {
        bucket_prefix_entries: HashMap<usize, NodeIdSubnet>,
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
/// - For every [Contact] in the [RoutingTable] a [NodeIdEntry] exists.
/// - For every [Contact] which is not a underlay neighbor a [PathIdEntry] exists.
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
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    fn remove_node_id_entry(
        &mut self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<(), DeriveFwdEntriesError> {
        let DeriveFwdTableEntriesState::Running {
            bucket_prefix_entries,
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
        if bucket_prefix_entries
            .get(&bucket_index)
            .is_some_and(|prefix_entry| prefix_entry.node_id() != node_id)
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
        let entry = self.derive_node_id_entry(context, &contact)?;
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
        let entry = self.derive_node_id_entry(context, &contact)?;
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
        let Some(prefix_entry) = self.derive_bucket_prefix_entry(context, bucket_index)? else {
            let DeriveFwdTableEntriesState::Running {
                bucket_prefix_entries,
            } = &mut self.state
            else {
                panic!("update_bucket called without running DeriveFwdTableEntries");
            };

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
        } = &mut self.state
        else {
            panic!("update_bucket called without running DeriveFwdTableEntries");
        };

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
        let next_hop = context
            .uln_table()
            .get(&next_hop)
            .copied()
            .ok_or(DeriveFwdEntriesError::NeighborNotInULNTable(next_hop))?;

        if context.uln_table().contains_key(contact.id()) && !contact.is_uln() {
            log::warn!(target: "derive_fwd_table_entries", 
                "Contact {contact:?} is not a underlay neighbor but listed in ULNTable -> may overwrite previous route unintentionally!");
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
            Ok(NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination: NodeIdSubnet::new(*contact.id()),
                next_hop,
                out_path_id: self.config.hasher.hash(contact.path().unwrap()),
            }))
        }
    }

    /// Derives one prefix entry of the bucket using proximity neighbor selection.
    ///
    /// If the bucket is empty [None] is returned
    fn derive_bucket_prefix_entry(
        &self,
        context: &C,
        bucket_index: usize,
    ) -> Result<Option<NodeIdEntry>, DeriveFwdEntriesError> {
        let rt = context.routing_table();
        let bucket = rt.bucket_by_index(bucket_index);
        let iter = bucket.iter();
        let prefix_len = rt.get_bucket_prefix_length(bucket_index);

        // Proximity Neighbor Selection:
        // Select contact with shortest path in bucket as prefix entry
        let Some(closest) = iter.filter(|c| c.is_valid()).min_by_key(|c| {
            c.path()
                .expect("valid contact should have an active path")
                .size()
        }) else {
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
        let next_hop = context
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
            NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination: subnet,
                next_hop,
                out_path_id: self.config.hasher.hash(closest.path().unwrap()),
            })
        };

        tracing::trace!(
            target: "derive_fwd_table_entries",
            %bucket_index,
            ?bucket,
            %entry,
            "derived prefix entry"
        );

        Ok(Some(entry))
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = DeriveFwdEntriesError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "derive_fwd_table_entries",
        "derive_fwd_table_entries",
        skip(self, context),
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
        let UseCaseEvent::Contact(contact_event) = event else {
            return Ok(());
        };
        match *contact_event {
            // FIXME: create NodeId entry on receiving PathSetupRsp
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
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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
        };

        context
            .runtime()
            .update_fwd_tables(PathIdTableUpdate::Create(PathIdEntry::Decapsulate(entry)));

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
    #[display("Contact is not a underlay neighbor but listed in ULNTable: {_0}")]
    NonUNInULNTable(Box<Contact>),
}
impl Error for DeriveFwdEntriesError {}

#[cfg(test)]
mod tests {
    use crate::Output;
    use crate::context::ContextConfig;
    use crate::context::SyncContext;
    use crate::domain::SafeStateSeqNr;
    use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::runtime::testing::TestingUseCaseRuntime;

    use super::*;

    mod vicinity {
        //! Tests that DeriveFwdEntries will not change any paths inside the vicinity

        use crate::domain::ConnectionId;
        use crate::domain::InterfaceId;

        use super::*;

        #[test]
        fn vicinity_contact_added() {
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
            uln_table.insert(neighbor_id, ulnid);

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Sha1,
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
                //Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1))
                Hasher::Sha1.hash(vicinity_contact.path().unwrap().into_iter())
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
                "Addtional forwarding updates present: {fwd_updates:#?}"
            );
        }

        #[test]
        fn uln_contact_added() {
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
            uln_table.insert(neighbor_id, ulnid);

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Sha1,
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
                "Addtional forwarding updates present: {fwd_updates:#?}"
            );
        }

        #[test]
        fn vicinity_contact_removed() {
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
            uln_table.insert(neighbor_id, ulnid);

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Sha1,
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
                "Addtional forwarding updates present: {fwd_updates:#?}"
            );
        }

        #[test]
        /// Updates a contact inside the vicinity to a contact outside the vicinity.
        fn vicinity_contact_updated() {
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
            uln_table.insert(neighbor_id, ulnid);

            let sync_context = SyncContext::new(ContextConfig {
                root_id,
                routing_table,
                uln_table,
                insertion_strategy: (),
                runtime,
                vicinity_graph: (),
            });

            let config = DeriveFwdTableEntriesConfig {
                hasher: Hasher::Sha1,
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
            let new_out_path_id = Hasher::Sha1.hash(new_contact.path().unwrap());

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
                "Addtional forwarding updates present: {fwd_updates:#?}"
            );
        }
    }
}
