use derive_more::derive::Display;
use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::ops::Deref;

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
#[derive(Debug, Default)]
pub struct DeriveFwdTableEntriesConfig {
    /// Hashing algorithm to use.
    pub hasher: Hasher,
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
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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
            .pn_table()
            .get(&next_hop)
            .copied()
            .ok_or(DeriveFwdEntriesError::NeighborNotInPNTable(next_hop))?;

        if context.pn_table().contains_key(contact.id()) && !contact.is_pn() {
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

        if let Some(closest) = iter.min_by_key(|c| c.path().size()) {
            let subnet = NodeIdSubnet::try_new(closest.id().prefix(prefix_len), prefix_len)
                .expect("should be valid prefix length");
            log::trace!(target: "derive_fwd_table_entries", "derived prefix entry {:?} for contact {:?}", subnet, closest.path());

            let next_hop = *closest.path().first();
            let next_hop = context
                .pn_table()
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
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn remove_path_id_entry(&self, context: &C, contact: Contact) {
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
                .pn_table()
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
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = DeriveFwdEntriesError;
    type Value = ();

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
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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
