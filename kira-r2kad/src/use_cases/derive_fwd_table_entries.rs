use derive_more::derive::Display;
use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::ops::Deref;
use tracing::{instrument, Level};

use crate::domain::protocol_event::forwarding::{
    DecapsulationDestination, NodeIdEncapsulationEntry, NodeIdEntry, NodeIdForwardingEntry,
    NodeIdTableUpdate, PathIdDecapsulationEntry, PathIdEntry, PathIdForwardingEntry,
    PathIdTableUpdate,
};
use crate::domain::{
    Contact, ContactState, Hasher, NodeId, NodeIdSubnet, Path, RoutingTable, UnderlayNeighborId,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    ContactEvent, EventHandler, ReactiveUseCaseState, UseCase, UseCaseContext, UseCaseEvent,
};

/// Configuration for [DeriveFwdTableEntries] use case.
#[derive(Debug)]
pub struct DeriveFwdTableEntriesConfig {
    /// Hashing algorithm to use.
    pub hasher: Hasher,
    pub vicinity_radius: usize,
}

impl Default for DeriveFwdTableEntriesConfig {
    fn default() -> Self {
        Self {
            hasher: Hasher::default(),
            vicinity_radius: 3,
        }
    }
}

/// Use case to derive forwarding table entries.
///
/// ## Invariants
///
/// - For every [Contact] in the [RoutingTable] a [NodeIdEntry] exists.
/// - For every [Contact] which is not a underlay neighbor a [PathIdEntry] exists.
#[derive(Debug)]
pub struct DeriveFwdTableEntries<C, const BUCKET_SIZE: usize> {
    state: ReactiveUseCaseState,
    _pd: PhantomData<C>,
    config: DeriveFwdTableEntriesConfig,
}

impl<C, const BUCKET_SIZE: usize> DeriveFwdTableEntries<C, BUCKET_SIZE> {
    pub fn new(config: DeriveFwdTableEntriesConfig) -> Self {
        Self {
            state: ReactiveUseCaseState::Idle,
            _pd: PhantomData,
            config,
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
        &self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<(), DeriveFwdEntriesError> {
        let entry = NodeIdSubnet::new(*node_id);
        context
            .runtime()
            .update_fwd_tables(NodeIdTableUpdate::Remove(entry.clone()));

        // FIXME: check if entry was present before

        log::trace!(target: "derive_fwd_table_entries", "Removed entry {:?}", entry);

        if let Some(prefix_entry) = self.derive_prefix_entry(context, node_id)? {
            context
                .runtime()
                .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(prefix_entry));
        }

        Ok(())
    }

    fn create_node_id_entry(
        &self,
        context: &C,
        contact: Contact,
    ) -> Result<(), DeriveFwdEntriesError> {
        let entry = self.derive_node_id_entry(context, &contact)?;
        context
            .runtime()
            .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(entry));

        if let Some(prefix_entry) = self.derive_prefix_entry(context, contact.id())? {
            context
                .runtime()
                .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(prefix_entry));
        }
        Ok(())
    }

    fn update_node_id_entry(
        &self,
        context: &C,
        new_entry: Contact,
    ) -> Result<(), DeriveFwdEntriesError> {
        let entry = self.derive_node_id_entry(context, &new_entry)?;
        context
            .runtime()
            .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(entry));

        if let Some(prefix_entry) = self.derive_prefix_entry(context, new_entry.id())? {
            context
                .runtime()
                .update_fwd_tables(NodeIdTableUpdate::CreateOrUpdate(prefix_entry));
        }
        Ok(())
    }

    fn update_bucket(&self, context: &C, bucket_index: usize) -> Result<(), DeriveFwdEntriesError> {
        let rt = context.routing_table();
        let iter = rt.bucket_by_index(bucket_index).iter();
        for contact in iter {
            self.update_node_id_entry(context, contact.clone())?;
        }

        Ok(())
    }

    fn derive_node_id_entry(
        &self,
        context: &C,
        contact: &Contact,
    ) -> Result<NodeIdEntry, DeriveFwdEntriesError> {
        let next_hop = *contact.path().first();
        let next_hop = context
            .un_table()
            .get(&next_hop)
            .copied()
            .ok_or(DeriveFwdEntriesError::NeighborNotInPNTable(next_hop))?;

        if context.un_table().contains_key(contact.id()) && !contact.is_pn() {
            log::warn!(target: "derive_fwd_table_entries", 
                "Contact {:?} is not a underlay neighbor but listed in PNTable -> may overwrite previous route unintentionally!", 
                contact);
            return Err(DeriveFwdEntriesError::NonPNInPNTable(contact.clone()));
        }

        if contact.is_pn() {
            Ok(NodeIdEntry::Forward(NodeIdForwardingEntry {
                destination: NodeIdSubnet::new(*contact.id()),
                next_hop,
            }))
        } else {
            Ok(NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination: NodeIdSubnet::new(*contact.id()),
                next_hop,
                out_path_id: self.config.hasher.hash(contact.path()),
            }))
        }
    }

    fn derive_prefix_entry(
        &self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<Option<NodeIdEntry>, DeriveFwdEntriesError> {
        let rt = context.routing_table();
        let bucket_index = rt.get_bucket_index(node_id);
        let iter = rt.bucket_by_index(bucket_index).iter();
        let prefix_len = rt.get_bucket_prefix_length(bucket_index);

        if let Some(closest) = iter
            .filter(|c| c.id() != context.root_id())
            .min_by_key(|c| c.path().size())
        {
            let subnet = NodeIdSubnet::try_new(closest.id().prefix(prefix_len), prefix_len)
                .expect("should be valid prefix length");
            log::trace!(target: "derive_fwd_table_entries", "derived prefix entry {:?} for contact {:?}", subnet, closest.path());

            let next_hop = *closest.path().first();
            let next_hop = context
                .un_table()
                .get(&next_hop)
                .copied()
                .ok_or(DeriveFwdEntriesError::NeighborNotInPNTable(next_hop))?;

            if closest.is_pn() {
                return Ok(Some(NodeIdEntry::Forward(NodeIdForwardingEntry {
                    destination: subnet,
                    next_hop,
                })));
            } else {
                return Ok(Some(NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                    destination: subnet,
                    next_hop,
                    out_path_id: self.config.hasher.hash(closest.path()),
                })));
            }
        }
        Ok(None)
    }
}

impl<C, const BUCKET_SIZE: usize> DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn remove_path_id_entry(&self, context: &C, contact: Contact) {
        if contact.path().size() < self.config.vicinity_radius {
            // Paths must be present even if the Node isn't in the RoutingTable
            // PrecomputePathIds will handle deletion if Node moves out of Vicinity
            return;
        }

        let in_path_id = self.config.hasher.hash(contact.path());
        context
            .runtime()
            .update_fwd_tables(PathIdTableUpdate::Remove(in_path_id));
    }

    fn create_path_id_entry(
        &self,
        context: &C,
        contact: Contact,
    ) -> Result<(), DeriveFwdEntriesError> {
        if contact.path().size() < self.config.vicinity_radius {
            // Path in Vicinity is already precomputed and set up by PrecomputePathIds
            return Ok(());
        }
        let entry = self.derive_path_id_entry(context, contact)?;
        context
            .runtime()
            .update_fwd_tables(PathIdTableUpdate::CreateOrUpdate(entry));

        Ok(())
    }

    // Updates the path id entry for a contact.
    fn update_path_id_entry(
        &self,
        context: &C,
        old_entry: Contact,
        new_entry: Contact,
    ) -> Result<(), DeriveFwdEntriesError> {
        if new_entry.path().size() < self.config.vicinity_radius {
            // Path in Vicinity is already precomputed and set up by PrecomputePathIds

            // Cleanup previous employed path
            self.remove_path_id_entry(context, old_entry);
            return Ok(());
        }

        if old_entry.path().size() < self.config.vicinity_radius
            && old_entry.state() == &ContactState::Valid
        {
            // Don't overwrite Path in Vicinity managed by PrecomputePathIds
            return Ok(());
        }

        if old_entry.path() != new_entry.path() {
            let entry = self.derive_path_id_entry(context, new_entry)?;
            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::CreateOrUpdate(entry));
        }

        Ok(())
    }

    fn derive_path_id_entry(
        &self,
        context: &C,
        contact: Contact,
    ) -> Result<PathIdEntry, DeriveFwdEntriesError> {
        let mut in_path = Path::from(*context.root_id());
        in_path.extend(contact.path().clone());
        let in_path_id = self.config.hasher.hash(&in_path);

        // Note: currently we follow the complete path and don't decapsulate early:
        // This is because the inner destination could be not a underlay neighbor,
        // which would cause the packet to be rerouted according to the
        // locally installed routes instead of being forwarded to the next hop of the path.

        // As soon as a solution is found for forwarding early decapsulated packets
        // to the next hop as determined by the path instead of treating them like
        // locally generated packets, the following line can be uncommented to allow
        // enabling early decapsulation.

        // if contact.id() == context.root_id() || contact.is_pn() {
        if contact.id() == context.root_id() {
            let result = PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                in_path_id,
                next_hop: DecapsulationDestination::Local,
            });

            log::debug!(target: "derive_fwd_table_entries", "Derived PathIdEntry {:?}", result);
            Ok(result)
        } else {
            let next_hop = *contact.path().first();
            let next_hop = context
                .un_table()
                .get(&next_hop)
                .copied()
                .ok_or(DeriveFwdEntriesError::NeighborNotInPNTable(next_hop))?;
            let out_path_id = self.config.hasher.hash(contact.path());

            let result = PathIdEntry::Forward(PathIdForwardingEntry {
                in_path_id,
                out_path_id,
                next_hop,
            });

            log::debug!(target: "derive_fwd_table_entries", "Derived PathIdEntry {:?}", result);
            Ok(result)
        }
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
        match event {
            UseCaseEvent::Contact(ContactEvent::New(contact)) => {
                self.create_node_id_entry(context, contact.clone())?;
                self.create_path_id_entry(context, contact)?;
            }
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                self.remove_node_id_entry(context, contact.id())?;
                self.remove_path_id_entry(context, contact);
            }
            UseCaseEvent::Contact(ContactEvent::Updated { new, old }) => {
                if &ContactState::Valid != new.state() && &ContactState::Valid == old.state() {
                    log::debug!(target: "derive_fwd_table_entries", "Contact {:?} changed to invalid state", new);
                    // if changed to invalid state => remove
                    self.remove_node_id_entry(context, new.id())?;
                    self.remove_path_id_entry(context, new);
                } else if new.state() == &ContactState::Valid && old.state() != &ContactState::Valid
                {
                    // If changed back to valid state => create new
                    self.create_node_id_entry(context, new.clone())?;
                    self.create_path_id_entry(context, new)?;
                } else if new.state() == &ContactState::Valid
                    && old.state() == &ContactState::Valid
                    && new.path() != old.path()
                {
                    // If both are valid => Just update existing entry
                    self.update_node_id_entry(context, new.clone())?;
                    self.update_path_id_entry(context, old, new)?;
                }
            }
            UseCaseEvent::Contact(ContactEvent::BucketUpdated(bucket)) => {
                self.update_bucket(context, bucket)?;
            }
            UseCaseEvent::Contact(ContactEvent::NewBucket(bucket)) => {
                self.update_bucket(context, bucket)?;
            }
            _ => {}
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
    type State = ReactiveUseCaseState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let in_path = Path::from(*context.root_id());
        let in_path_id = self.config.hasher.hash(&in_path);
        let entry = PathIdDecapsulationEntry {
            in_path_id,
            next_hop: DecapsulationDestination::Local,
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
    #[display("Neighbor listed in contacts path not in PNTable: {_0}")]
    NeighborNotInPNTable(NodeId),
    #[display("Contact is not a underlay neighbor but listed in PNTable: {_0}")]
    NonPNInPNTable(Contact),
}
impl Error for DeriveFwdEntriesError {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::context::ContextConfig;
    use crate::context::SyncContext;
    use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::StateSeqNr;
    use crate::domain::SIZE;
    use crate::runtime::testing::TestingUseCaseRuntime;
    use crate::Output;

    use super::*;

    #[test]
    fn contact_entries_added() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();

        let ulnid = UnderlayNeighborId::try_from(1).unwrap();
        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let vicinity_contact_id = NodeId::with_lsb(3);

        let neighbor = Contact::new(Path::from(neighbor_id), StateSeqNr::from(0));
        let vicinity_contact = Contact::new(
            Path::from([neighbor_id, vicinity_contact_id]),
            StateSeqNr::from(3),
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
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
            vicinity_radius: 3,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");
        let _ = sync_context.runtime().output();

        let event = UseCaseEvent::Contact(ContactEvent::New(vicinity_contact.clone()));
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Error handling event: {:?}",
            handle_result
        );

        let updates: Vec<_> = sync_context.runtime().output().collect();

        let node_id_update = updates
            .iter()
            .find_map(|u| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::NodeIdTableUpdate(
                    node_id_update,
                )) = u
                {
                    Some(node_id_update)
                } else {
                    None
                }
            })
            .expect("No node id table update generated for added contact");
        let NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Encapsulate(node_id_entry)) =
            node_id_update
        else {
            panic!("unexpected kind of node id table update: {node_id_update:?}");
        };
        assert_eq!(node_id_entry.destination.node_id(), vicinity_contact.id(),);
        assert!(
            node_id_entry.destination.prefix_length() == 0
                || node_id_entry.destination.prefix_length() == SIZE,
        );
        assert_eq!(node_id_entry.next_hop, ulnid);
        assert_eq!(
            node_id_entry.out_path_id,
            //Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1))
            Hasher::Sha1.hash(vicinity_contact.path().into_iter())
        );

        let path_id_update = updates
            .iter()
            .find_map(|u| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    path_id_update,
                )) = u
                {
                    Some(path_id_update)
                } else {
                    None
                }
            })
            .expect("No node id table update generated for added contact");
        let PathIdTableUpdate::CreateOrUpdate(PathIdEntry::Forward(path_id_entry)) = path_id_update
        else {
            panic!("unexpected kind of path id table update: {path_id_update:?}");
        };

        let mut in_path = Path::from(root_id);
        in_path.extend(vicinity_contact.path().clone());
        let in_path_id = Hasher::Sha1.hash(&in_path);
        assert_eq!(&path_id_entry.in_path_id, &in_path_id);

        assert_eq!(
            path_id_entry.out_path_id,
            //Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1))
            Hasher::Sha1.hash(vicinity_contact.path().into_iter())
        );
        assert_eq!(path_id_entry.next_hop, ulnid);
    }

    #[test]
    fn neighbor_entries_added() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();

        let ulnid = UnderlayNeighborId::try_from(1).unwrap();
        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);

        let neighbor = Contact::new(Path::from(neighbor_id), StateSeqNr::from(0));

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
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
            vicinity_radius: 3,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");
        let _ = sync_context.runtime().output();

        let event = UseCaseEvent::Contact(ContactEvent::New(neighbor.clone()));
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Error handling event: {:?}",
            handle_result
        );

        let updates: Vec<_> = sync_context.runtime().output().collect();

        let node_id_update = updates
            .iter()
            .find_map(|u| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::NodeIdTableUpdate(
                    node_id_update,
                )) = u
                {
                    Some(node_id_update)
                } else {
                    None
                }
            })
            .expect("No node id table update generated for added contact");
        let NodeIdTableUpdate::CreateOrUpdate(NodeIdEntry::Forward(node_id_entry)) = node_id_update
        else {
            panic!("unexpected kind of node id table update: {node_id_update:?}");
        };
        assert!(
            node_id_entry.destination.prefix_length() == 0
                || node_id_entry.destination.prefix_length() == SIZE,
        );
        assert_eq!(node_id_entry.next_hop, ulnid);

        assert!(
            updates.iter().any(|u| {
                matches!(
                    u,
                    Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(_)),
                )
            }),
            "No PathID entry should be created for neighbor"
        );
    }

    #[test]
    fn contact_entries_removed() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();

        let ulnid = UnderlayNeighborId::try_from(1).unwrap();
        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let vicinity_contact_id = NodeId::with_lsb(3);

        let neighbor = Contact::new(Path::from(neighbor_id), StateSeqNr::from(0));
        let vicinity_contact = Contact::new(
            Path::from([neighbor_id, vicinity_contact_id]),
            StateSeqNr::from(3),
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
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
            vicinity_radius: 3,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");
        let _ = sync_context.runtime().output();

        let in_path_id = Hasher::Sha1.hash(vicinity_contact.path());

        let event = UseCaseEvent::Contact(ContactEvent::Removed(vicinity_contact.clone()));
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Error handling event: {:?}",
            handle_result
        );

        let updates: Vec<_> = sync_context.runtime().output().collect();

        let node_id_update = updates
            .iter()
            .find_map(|u| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::NodeIdTableUpdate(
                    node_id_update,
                )) = u
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

        let path_id_update = updates
            .iter()
            .find_map(|u| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    path_id_update,
                )) = u
                {
                    Some(path_id_update)
                } else {
                    None
                }
            })
            .expect("No removal of path id table entry");
        let PathIdTableUpdate::Remove(removed_pid) = path_id_update else {
            panic!("unexpected kind of path id table update: {path_id_update:?}");
        };
        assert_eq!(*removed_pid, in_path_id);

        {
            let event = UseCaseEvent::Contact(ContactEvent::Removed(neighbor.clone()));
            let handle_result = use_case.handle_event(&sync_context, event);
            assert!(
                handle_result.is_ok(),
                "Error handling event: {:?}",
                handle_result
            );

            let updates: Vec<_> = sync_context.runtime().output().collect();

            let node_id_update = updates
                .iter()
                .find_map(|u| {
                    if let Output::UpdateForwardingTables(
                        ForwardingTablesUpdate::NodeIdTableUpdate(node_id_update),
                    ) = u
                    {
                        Some(node_id_update)
                    } else {
                        None
                    }
                })
                .expect("No node id table update generated for removed contact");
            let NodeIdTableUpdate::Remove(removed_subnet) = node_id_update else {
                panic!("unexpected kind of node id table update: {node_id_update:?}");
            };
            assert_eq!(removed_subnet.node_id(), neighbor.id(),);
        }
    }

    #[test]
    fn contact_updated() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();

        let ulnid = UnderlayNeighborId::try_from(1).unwrap();
        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let vicinity_contact_id = NodeId::with_lsb(3);

        let neighbor = Contact::new(Path::from(neighbor_id), StateSeqNr::from(0));
        let vicinity_contact = Contact::new(
            Path::from([neighbor_id, vicinity_contact_id]),
            StateSeqNr::from(3),
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
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
            vicinity_radius: 3,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");
        let _ = sync_context.runtime().output();

        let in_path_id = Hasher::Sha1.hash(vicinity_contact.path());

        let new_contact = Contact::new(
            Path::from([
                neighbor_id,
                NodeId::with_lsb(4),
                NodeId::with_lsb(5),
                vicinity_contact_id,
            ]),
            StateSeqNr::from(15),
        );

        let mut new_in_path = Path::from(root_id);
        new_in_path.extend(new_contact.path().clone());
        let new_in_path_id = Hasher::Sha1.hash(&new_in_path);
        let new_out_path_id = Hasher::Sha1.hash(new_contact.path());

        let event = UseCaseEvent::Contact(ContactEvent::Updated {
            old: vicinity_contact.clone(),
            new: new_contact.clone(),
        });
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Error handling event: {:?}",
            handle_result
        );

        // Node id entry is updated
        let updates: Vec<_> = sync_context.runtime().output().collect();

        let node_id_update = updates
            .iter()
            .find_map(|u| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::NodeIdTableUpdate(
                    node_id_update,
                )) = u
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
        assert_eq!(node_id_entry.destination.node_id(), vicinity_contact.id(),);
        assert!(
            node_id_entry.destination.prefix_length() == 0
                || node_id_entry.destination.prefix_length() == SIZE,
        );
        assert_eq!(node_id_entry.next_hop, ulnid);
        assert_eq!(node_id_entry.out_path_id, new_out_path_id,);

        // Old pathid entry is removed
        let removed_pid = updates.iter().find_map(|u| {
            if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                PathIdTableUpdate::Remove(removed_pid),
            )) = u
            {
                Some(removed_pid)
            } else {
                None
            }
        });
        assert!(
            removed_pid.is_some(),
            "No removal of path id entry on changed contact: {updates:?}"
        );
        let removed_pid = removed_pid.unwrap();
        assert_eq!(*removed_pid, in_path_id);

        // new path id entry is created
        let path_id_entry = updates
            .iter()
            .find_map(|u| {
                if let Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    PathIdTableUpdate::CreateOrUpdate(PathIdEntry::Forward(path_id_entry)),
                )) = u
                {
                    Some(path_id_entry)
                } else {
                    None
                }
            })
            .expect("No path id table update generated for changed contact");
        assert_eq!(path_id_entry.in_path_id, new_in_path_id);
        assert_eq!(path_id_entry.out_path_id, new_out_path_id);
        assert_eq!(path_id_entry.next_hop, ulnid);
    }
}
