use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::context::UseCaseContext;
use crate::domain::{
    Contact, ContactState, NetworkInterface, NodeId, NodeIdSubnet, Path, RoutingTable,
};
use crate::forwarding::hasher::Hasher;
use crate::forwarding::{
    ForwardingTables, NodeIdEncapsulationEntry, NodeIdEntry, NodeIdForwardingEntry, NodeIdTable,
    PathIdDecapsulationEntry, PathIdEntry, PathIdForwardingEntry, PathIdTable,
};
use crate::use_cases::{ContactEvent, EventHandler, ReactiveUseCaseState, UseCase, UseCaseEvent};

/// Configuration for [DeriveFwdTableEntries] use case.
#[derive(Debug, Default)]
pub struct DeriveFwdTableEntriesConfig {
    /// Hashing algorithm to use.
    pub hasher: Hasher,
}

/// Use case to derive forwarding table entries through the [ForwardingTables] trait.
///
/// ## Invariants
///
/// - For every [Contact] in the [RoutingTable](crate::domain::routing_table::RoutingTable) a [NodeIdEntry] exists.
/// - For every [Contact] which is not a physical neighbor a [PathIdEntry] exists.
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
            _pd: PhantomData::default(),
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::ForwardingTables: NodeIdTable,
    <C::ForwardingTables as NodeIdTable>::Error: 'static + Error,
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, NetworkInterface>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    fn remove_node_id_entry(
        &self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<(), error::DeriveFwdEntriesError> {
        let mut fw_tables = context.forwarding_tables_mut();
        let entry = fw_tables
            .remove(&NodeIdSubnet {
                node_id: node_id.clone(),
                prefix_length: 0,
            })
            .map_err(|e| error::DeriveFwdEntriesError::NodeIdTable(Box::new(e)))?;
        if let Some(entry) = entry {
            log::trace!(target: "derive_fwd_table_entries", "Removed entry {:?}", entry);

            if let Some(prefix_entry) = self.derive_prefix_entry(context, node_id)? {
                fw_tables
                    .create_or_update(prefix_entry)
                    .map_err(|e| error::DeriveFwdEntriesError::NodeIdTable(Box::new(e)))?;
            }
        }

        Ok(())
    }

    fn create_node_id_entry(
        &self,
        context: &C,
        contact: Contact,
    ) -> Result<(), error::DeriveFwdEntriesError> {
        let entry = self.derive_node_id_entry(context, &contact)?;

        let mut fw_tables = context.forwarding_tables_mut();
        fw_tables
            .create_or_update(entry)
            .map_err(|e| error::DeriveFwdEntriesError::NodeIdTable(Box::new(e)))?;

        if let Some(prefix_entry) = self.derive_prefix_entry(context, contact.id())? {
            fw_tables
                .create_or_update(prefix_entry)
                .map_err(|e| error::DeriveFwdEntriesError::NodeIdTable(Box::new(e)))?;
        }
        Ok(())
    }

    fn update_node_id_entry(
        &self,
        context: &C,
        new_entry: Contact,
    ) -> Result<(), error::DeriveFwdEntriesError> {
        let entry = self.derive_node_id_entry(context, &new_entry)?;

        let mut fw_tables = context.forwarding_tables_mut();
        fw_tables
            .create_or_update(entry)
            .map_err(|e| error::DeriveFwdEntriesError::NodeIdTable(Box::new(e)))?;

        if let Some(prefix_entry) = self.derive_prefix_entry(context, new_entry.id())? {
            fw_tables
                .create_or_update(prefix_entry)
                .map_err(|e| error::DeriveFwdEntriesError::NodeIdTable(Box::new(e)))?;
        }
        Ok(())
    }

    fn update_bucket(
        &self,
        context: &C,
        bucket_index: usize,
    ) -> Result<(), error::DeriveFwdEntriesError> {
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
    ) -> Result<NodeIdEntry, error::DeriveFwdEntriesError> {
        let next_hop = contact.path().first().clone();
        let out_interface =
            context.pn_table().get(&next_hop).cloned().ok_or_else(|| {
                error::DeriveFwdEntriesError::NeighborNotInPNTable(next_hop.clone())
            })?;

        if contact.is_pn() {
            Ok(NodeIdEntry::Forward(NodeIdForwardingEntry {
                destination: NodeIdSubnet {
                    node_id: contact.id().clone(),
                    prefix_length: 0,
                },
                next_hop,
                out_interface,
            }))
        } else {
            Ok(NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination: NodeIdSubnet {
                    node_id: contact.id().clone(),
                    prefix_length: 0,
                },
                next_hop,
                out_interface,
                out_path_id: self.config.hasher.hash(contact.path().into_iter()),
            }))
        }
    }

    fn derive_prefix_entry(
        &self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<Option<NodeIdEntry>, error::DeriveFwdEntriesError> {
        let rt = context.routing_table();
        let bucket_index = rt.get_bucket_index(node_id);
        let iter = rt.bucket_by_index(bucket_index).iter();
        let prefix_len = rt.get_bucket_prefix_length(bucket_index);

        if let Some(closest) = iter.min_by_key(|c| c.path().size()) {
            let subnet = NodeIdSubnet {
                node_id: closest.id().prefix(prefix_len),
                prefix_length: prefix_len,
            };
            log::trace!(target: "derive_fwd_table_entries", "derived prefix entry {:?} for contact {:?}", subnet, closest.path());

            let next_hop = closest.path().first().clone();
            let out_interface = context.pn_table().get(&next_hop).cloned().ok_or_else(|| {
                error::DeriveFwdEntriesError::NeighborNotInPNTable(next_hop.clone())
            })?;

            if closest.is_pn() {
                return Ok(Some(NodeIdEntry::Forward(NodeIdForwardingEntry {
                    destination: subnet,
                    next_hop,
                    out_interface,
                })));
            } else {
                return Ok(Some(NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                    destination: subnet,
                    next_hop,
                    out_path_id: self.config.hasher.hash(closest.path().into_iter()),
                    out_interface,
                })));
            }
        }
        Ok(None)
    }
}

impl<C, const BUCKET_SIZE: usize> DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::ForwardingTables: PathIdTable,
    <C::ForwardingTables as PathIdTable>::Error: 'static + Error,
{
    fn remove_path_id_entry(
        &self,
        context: &C,
        contact: Contact,
    ) -> Result<(), error::DeriveFwdEntriesError> {
        let in_path_id = self.config.hasher.hash(contact.path());

        context
            .forwarding_tables_mut()
            .remove(&in_path_id)
            .map_err(|e| error::DeriveFwdEntriesError::PathIdTable(Box::new(e)))?;

        Ok(())
    }

    fn create_path_id_entry(
        &self,
        context: &C,
        contact: Contact,
    ) -> Result<(), error::DeriveFwdEntriesError> {
        let entry = self.derive_path_id_entry(context, contact);
        context
            .forwarding_tables_mut()
            .create_or_update(entry)
            .map_err(|e| error::DeriveFwdEntriesError::PathIdTable(Box::new(e)))?;

        Ok(())
    }

    // Updates the path id entry for a contact.
    fn update_path_id_entry(
        &self,
        context: &C,
        old_entry: Contact,
        new_entry: Contact,
    ) -> Result<(), error::DeriveFwdEntriesError> {
        let mut forwarding_table = context.forwarding_tables_mut();

        if old_entry.path() != new_entry.path() {
            let entry = self.derive_path_id_entry(context, new_entry);
            forwarding_table
                .create_or_update(entry)
                .map_err(|e| error::DeriveFwdEntriesError::PathIdTable(Box::new(e)))
        } else {
            Ok(())
        }
    }

    fn derive_path_id_entry(&self, context: &C, contact: Contact) -> PathIdEntry {
        let mut in_path = Path::from(context.root_id().clone());
        in_path.extend(contact.path().clone());
        let in_path_id = self.config.hasher.hash(&in_path);

        // Note: currently we follow the complete path and don't decapsulate early:
        // This is because the inner destination could be not a physical neighbor, 
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
                local_id: contact.id().clone(),
            });

            log::debug!(target: "derive_fwd_table_entries", "Derived PathIdEntry {:?}", result);
            result
        } else {
            let next_hop = contact.path().first().clone();
            let out_path_id = self.config.hasher.hash(contact.path().into_iter());

            let result = PathIdEntry::Forward(PathIdForwardingEntry {
                in_path_id,
                out_path_id,
                next_hop,
            });

            log::debug!(target: "derive_fwd_table_entries", "Derived PathIdEntry {:?}", result);
            result
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for DeriveFwdTableEntries<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::ForwardingTables: ForwardingTables,
    <C::ForwardingTables as NodeIdTable>::Error: 'static + Error,
    <C::ForwardingTables as PathIdTable>::Error: 'static + Error,
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, NetworkInterface>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = error::DeriveFwdEntriesError;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::Contact(ContactEvent::New(contact)) => {
                self.create_node_id_entry(context, contact.clone())?;
                self.create_path_id_entry(context, contact)?;
            }
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                self.remove_node_id_entry(context, contact.id())?;
                self.remove_path_id_entry(context, contact)?;
            }
            UseCaseEvent::Contact(ContactEvent::Updated { new, old }) => {
                if &ContactState::Valid != new.state() && &ContactState::Valid == old.state() {
                    // if changed to invalid state => remove
                    self.remove_node_id_entry(context, new.id())?;
                    self.remove_path_id_entry(context, new)?;
                } else if new.state() == &ContactState::Valid && old.state() != &ContactState::Valid
                {
                    // If changed back to valid state => create new
                    self.create_node_id_entry(context, new.clone())?;
                    self.create_path_id_entry(context, new)?;
                } else if new.state() == &ContactState::Valid && old.state() == &ContactState::Valid
                {
                    if new.path() != old.path() {
                        // If both are valid => Just update existing entry
                        self.update_node_id_entry(context, new.clone())?;
                        self.update_path_id_entry(context, old, new)?;
                    }
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
    C::ForwardingTables: ForwardingTables,
    <C::ForwardingTables as NodeIdTable>::Error: 'static + Error,
    <C::ForwardingTables as PathIdTable>::Error: 'static + Error,
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, NetworkInterface>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let in_path = Path::from(context.root_id().clone());
        let in_path_id = self.config.hasher.hash(&in_path);
        let entry = PathIdDecapsulationEntry {
            in_path_id,
            local_id: context.root_id().clone(),
        };
        let mut fw_tables = context.forwarding_tables_mut();
        PathIdTable::create(fw_tables.deref_mut(), PathIdEntry::Decapsulate(entry))
            .map_err(|e| error::DeriveFwdEntriesError::PathIdTable(Box::new(e)))
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

pub mod error {
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    use crate::domain::NodeId;

    #[derive(Debug)]
    pub enum DeriveFwdEntriesError {
        NodeIdTable(Box<dyn Error>),
        PathIdTable(Box<dyn Error>),
        NeighborNotInPNTable(NodeId),
    }

    impl Display for DeriveFwdEntriesError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::NodeIdTable(e) => write!(f, "Node ID Table error: {}", e),
                Self::PathIdTable(e) => write!(f, "Path ID Table error: {}", e),
                Self::NeighborNotInPNTable(id) => {
                    write!(f, "Neighbor listed in contacts path not in PNTable: {}", id)
                }
            }
        }
    }

    impl Error for DeriveFwdEntriesError {}
}

#[cfg(test)]
mod tests {
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{Contact, InsertionStrategyResult, NetworkInterface, NodeId, PNTable, Path, RoutingTable, StateSeqNr, TestInsertionStrategy, InMemoryPNTable};
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::forwarding::{NodeIdEntry, NodeIdTable, PathIdEntry, PathIdTable};
    use crate::messaging::InMemoryMessageChannel;
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::derive_fwd_table_entries::{
        DeriveFwdTableEntries, DeriveFwdTableEntriesConfig, Hasher,
    };
    use crate::use_cases::{ContactEvent, EventHandler, UseCase, UseCaseEvent};
    use std::collections::HashSet;

    #[test]
    fn contact_entries_added() {
        crate::tests::init();

        let interface = NetworkInterface::new(0);

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let vicinity_contact_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let vicinity_contact = Contact::new(
            Path::from([neighbor_id.clone(), vicinity_contact_id.clone()]),
            StateSeqNr::from(3),
        );

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");

        let event = UseCaseEvent::Contact(ContactEvent::New(vicinity_contact.clone()));
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Error handling event: {:?}",
            handle_result
        );

        let tables = sync_context.forwarding_tables();
        let node_id_entry = tables.node_id_entry(&vicinity_contact_id);
        assert!(
            node_id_entry.is_some(),
            "No node id entry created for added contact"
        );
        let node_id_entry = node_id_entry.unwrap();
        assert_eq!(&node_id_entry.destination, vicinity_contact.id());
        assert_eq!(&node_id_entry.next_hop, vicinity_contact.path().first());
        assert_eq!(&node_id_entry.out_interface, &interface);
        assert_eq!(
            node_id_entry.out_path_id.as_ref(),
            Some(&Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1)))
        );

        let path_id_in = Hasher::Sha1.hash(vicinity_contact.path());
        let path_id_entry = tables.path_id_entry(&path_id_in);
        assert!(
            path_id_entry.is_some(),
            "No path id entry created for added contact"
        );
        let path_id_entry = path_id_entry.unwrap();
        assert_eq!(&path_id_entry.in_path_id, &path_id_in);
        assert_eq!(
            path_id_entry.out_path_id.as_ref().unwrap(),
            &Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1))
        );
        //assert_eq!(&path_id_entry.out_interface, &interface);
    }

    #[test]
    fn neighbor_entries_added() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");

        let event = UseCaseEvent::Contact(ContactEvent::New(neighbor.clone()));
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Error handling event: {:?}",
            handle_result
        );

        let tables = sync_context.forwarding_tables();
        let node_id_entry = tables.node_id_entry(&neighbor_id);
        assert!(
            node_id_entry.is_some(),
            "No node id entry created for added contact"
        );
        let node_id_entry = node_id_entry.unwrap();
        assert_eq!(&node_id_entry.destination, neighbor.id());
        assert_eq!(&node_id_entry.next_hop, neighbor.path().first());
        assert_eq!(&node_id_entry.out_interface, &interface);
        assert!(
            node_id_entry.out_path_id.is_none(),
            "neighbors shouldn't have any outgoing path id"
        );

        let path_id_in = Hasher::Sha1.hash(neighbor.path());
        let path_id_entry = tables.path_id_entry(&path_id_in);
        assert!(
            path_id_entry.is_none(),
            "No PathID entry should be created for neighbor"
        );
    }

    #[test]
    fn contact_entries_removed() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let vicinity_contact_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let vicinity_contact = Contact::new(
            Path::from([neighbor_id.clone(), vicinity_contact_id.clone()]),
            StateSeqNr::from(3),
        );
        let in_path_id = Hasher::Sha1.hash(vicinity_contact.path());
        let out_path_id = Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let mut fwd_table = InMemoryFwdTables::new();
        assert!(
            NodeIdTable::create(
                &mut fwd_table,
                NodeIdEntry {
                    destination: neighbor.id().clone(),
                    prefix_len: 0,
                    next_hop: neighbor.path().first().clone(),
                    out_path_id: None,
                    out_interface: interface.clone(),
                },
            )
            .is_ok(),
            "failed to create neighbors node id entry"
        );
        assert!(
            NodeIdTable::create(
                &mut fwd_table,
                NodeIdEntry {
                    destination: vicinity_contact_id.clone(),
                    prefix_len: 0,
                    next_hop: vicinity_contact.path().first().clone(),
                    out_path_id: Some(out_path_id.clone()),
                    out_interface: interface.clone(),
                },
            )
            .is_ok(),
            "failed to create vicinity contacts node id entry"
        );
        assert!(
            PathIdTable::create(
                &mut fwd_table,
                PathIdEntry {
                    in_path_id: in_path_id.clone(),
                    out_path_id: Some(out_path_id),
                    next_hop: vicinity_contact.path().first().clone(),
                },
            )
            .is_ok(),
            "failed to create vicinity contacts path id entry"
        );

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: fwd_table,
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");

        let event = UseCaseEvent::Contact(ContactEvent::Removed(vicinity_contact.clone()));
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Error handling event: {:?}",
            handle_result
        );

        {
            let tables = sync_context.forwarding_tables();
            let node_id_entry = tables.node_id_entry(&vicinity_contact_id);
            assert!(
                node_id_entry.is_none(),
                "Node id entry should be removed but was: {:?}",
                node_id_entry
            );

            let path_id_entry = tables.path_id_entry(&in_path_id);
            assert!(
                path_id_entry.is_none(),
                "Path id entry should be removed but was: {:?}",
                path_id_entry
            );
        }

        {
            let event = UseCaseEvent::Contact(ContactEvent::Removed(neighbor.clone()));
            let handle_result = use_case.handle_event(&sync_context, event);
            assert!(
                handle_result.is_ok(),
                "Error handling event: {:?}",
                handle_result
            );

            let tables = sync_context.forwarding_tables();
            let node_id_entry = tables.node_id_entry(&neighbor_id);
            assert!(
                node_id_entry.is_none(),
                "Node id entry should be removed but was: {:?}",
                node_id_entry
            );
        }
    }

    #[test]
    fn contact_updated() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let vicinity_contact_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let vicinity_contact = Contact::new(
            Path::from([neighbor_id.clone(), vicinity_contact_id.clone()]),
            StateSeqNr::from(3),
        );
        let in_path_id = Hasher::Sha1.hash(vicinity_contact.path());
        let out_path_id = Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let mut fwd_table = InMemoryFwdTables::new();
        assert!(
            NodeIdTable::create(
                &mut fwd_table,
                NodeIdEntry {
                    destination: neighbor.id().clone(),
                    prefix_len: 0,
                    next_hop: neighbor.path().first().clone(),
                    out_path_id: None,
                    out_interface: interface.clone(),
                },
            )
            .is_ok(),
            "failed to create neighbors node id entry"
        );
        assert!(
            NodeIdTable::create(
                &mut fwd_table,
                NodeIdEntry {
                    destination: vicinity_contact_id.clone(),
                    prefix_len: 0,
                    next_hop: vicinity_contact.path().first().clone(),
                    out_path_id: Some(out_path_id.clone()),
                    out_interface: interface.clone(),
                },
            )
            .is_ok(),
            "failed to create vicinity contacts node id entry"
        );
        assert!(
            PathIdTable::create(
                &mut fwd_table,
                PathIdEntry {
                    in_path_id: in_path_id.clone(),
                    out_path_id: Some(out_path_id),
                    next_hop: vicinity_contact.path().first().clone()
                },
            )
            .is_ok(),
            "failed to create vicinity contacts path id entry"
        );

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: fwd_table,
            not_via: HashSet::default(),
        });

        let config = DeriveFwdTableEntriesConfig {
            hasher: Hasher::Sha1,
        };
        let mut use_case = DeriveFwdTableEntries::new(config);
        assert!(use_case.start(&sync_context).is_ok(), "starting failed");

        let new_contact = Contact::new(
            Path::from([
                neighbor_id.clone(),
                NodeId::with_lsb(4),
                NodeId::with_lsb(5),
                vicinity_contact_id.clone(),
            ]),
            StateSeqNr::from(15),
        );
        let new_in_path_id = Hasher::Sha1.hash(new_contact.path());
        let new_out_path_id = Hasher::Sha1.hash(new_contact.path().into_iter().skip(1));

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
        let fwd_tables = sync_context.forwarding_tables();
        let node_id_entry = fwd_tables.node_id_entry(&vicinity_contact_id);
        assert!(node_id_entry.is_some(), "Node id entry should still exist");
        let node_id_entry = node_id_entry.unwrap();
        assert_eq!(&node_id_entry.destination, &vicinity_contact_id);
        assert_eq!(&node_id_entry.next_hop, &neighbor_id);
        assert_eq!(&node_id_entry.out_path_id, &Some(new_out_path_id.clone()));
        assert_eq!(&node_id_entry.out_interface, &interface);

        // Old pathid entry is removed
        let path_id_entry = fwd_tables.path_id_entry(&in_path_id);
        assert!(
            path_id_entry.is_none(),
            "Path id entry should be removed but was: {:?}",
            path_id_entry
        );

        // new path id entry is created
        let path_id_entry = fwd_tables.path_id_entry(&new_in_path_id);
        assert!(
            path_id_entry.is_some(),
            "No path id entry created for added contact"
        );
        let path_id_entry = path_id_entry.unwrap();
        assert_eq!(&path_id_entry.in_path_id, &new_in_path_id);
        assert_eq!(path_id_entry.out_path_id.as_ref().unwrap(), &new_out_path_id);
        //assert_eq!(&path_id_entry.out_interface, &interface);
    }
}
