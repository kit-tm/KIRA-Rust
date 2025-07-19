use derive_more::derive::Display;
use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::ops::Deref;
use tracing::{Level, instrument};

use crate::domain::protocol_event::forwarding::{
    DecapsulationDestination, NodeIdEncapsulationEntry, NodeIdEntry, NodeIdForwardingEntry,
    NodeIdTableUpdate, PathIdDecapsulationEntry, PathIdEntry, PathIdTableUpdate,
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

        log::trace!(target: "derive_fwd_table_entries", "Removed entry {entry:?}");

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
            .uln_table()
            .get(&next_hop)
            .copied()
            .ok_or(DeriveFwdEntriesError::NeighborNotInULNTable(next_hop))?;

        if context.uln_table().contains_key(contact.id()) && !contact.is_uln() {
            log::warn!(target: "derive_fwd_table_entries", 
                "Contact {contact:?} is not a underlay neighbor but listed in ULNTable -> may overwrite previous route unintentionally!");
            return Err(DeriveFwdEntriesError::NonUNInULNTable(contact.clone()));
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
                .uln_table()
                .get(&next_hop)
                .copied()
                .ok_or(DeriveFwdEntriesError::NeighborNotInULNTable(next_hop))?;

            if closest.is_uln() {
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
            // FIXME: create NodeId entry on receiving PathSetupRsp
            UseCaseEvent::Contact(ContactEvent::New(contact)) => {
                self.create_node_id_entry(context, contact.clone())?;
            }
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                self.remove_node_id_entry(context, contact.id())?;
            }
            UseCaseEvent::Contact(ContactEvent::Updated { new, old }) => {
                match (new.state(), old.state()) {
                    (ContactState::Invalid, ContactState::Valid) => {
                        tracing::debug!(
                            target: "derive_fwd_table_entries",
                            ?new, ?old,
                            "Contact changed to invalid state"
                        );
                        self.remove_node_id_entry(context, new.id())?;
                        self.remove_node_id_entry(context, old.id())?;
                    }
                    (ContactState::Valid, ContactState::Invalid) => {
                        tracing::debug!(
                            target: "derive_fwd_table_entries",
                            ?new, ?old,
                            "Contact changed to valid state"
                        );
                        self.create_node_id_entry(context, new)?;
                    }
                    (ContactState::Valid, ContactState::Valid) if new.path() != old.path() => {
                        tracing::debug!(
                            target: "derive_fwd_table_entries",
                            ?new, ?old,
                            "Contact changed path"
                        );
                        if new.id() == old.id() {
                            // just a simple path change to the same destination
                            self.update_node_id_entry(context, new)?;
                        } else {
                            // Contact got substituted for another destination
                            // probably due to proximity neighbor selection
                            self.remove_node_id_entry(context, old.id())?;
                            self.create_node_id_entry(context, new)?;
                        }
                    }
                    _ => {}
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
    #[display("Neighbor listed in contacts path not in ULNTable: {_0}")]
    NeighborNotInULNTable(NodeId),
    #[display("Contact is not a underlay neighbor but listed in ULNTable: {_0}")]
    NonUNInULNTable(Contact),
}
impl Error for DeriveFwdEntriesError {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::Output;
    use crate::context::ContextConfig;
    use crate::context::SyncContext;
    use crate::domain::SIZE;
    use crate::domain::StateSeqNr;
    use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::runtime::testing::TestingUseCaseRuntime;

    use super::*;

    mod vicinity {
        //! Tests that DeriveFwdEntries will not change any paths inside the vicinity

        use super::*;

        #[test]
        fn vicinity_contact_added() {
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
            };
            let mut use_case = DeriveFwdTableEntries::new(config);
            assert!(use_case.start(&sync_context).is_ok(), "starting failed");
            let _ = sync_context.runtime().output();

            let event = UseCaseEvent::Contact(ContactEvent::New(vicinity_contact.clone()));
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
                    || encap_entry.destination.prefix_length() == SIZE,
            );
            assert_eq!(encap_entry.next_hop, ulnid);
            assert_eq!(
                encap_entry.out_path_id,
                //Hasher::Sha1.hash(vicinity_contact.path().into_iter().skip(1))
                Hasher::Sha1.hash(vicinity_contact.path().into_iter())
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
            };
            let mut use_case = DeriveFwdTableEntries::new(config);
            assert!(use_case.start(&sync_context).is_ok(), "starting failed");
            let _ = sync_context.runtime().output();

            let event = UseCaseEvent::Contact(ContactEvent::New(neighbor.clone()));
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
                    || fwd_entry.destination.prefix_length() == SIZE,
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
            };
            let mut use_case = DeriveFwdTableEntries::new(config);
            assert!(use_case.start(&sync_context).is_ok(), "starting failed");
            let _ = sync_context.runtime().output();

            let event = UseCaseEvent::Contact(ContactEvent::Removed(vicinity_contact.clone()));
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
                StateSeqNr::from(15),
            );

            let mut new_in_path = Path::from(root_id);
            new_in_path.extend(new_contact.path().clone());
            let new_out_path_id = Hasher::Sha1.hash(new_contact.path());

            let event = UseCaseEvent::Contact(ContactEvent::Updated {
                new: new_contact.clone(),
                old: vicinity_contact.clone(),
            });
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
                    || node_id_entry.destination.prefix_length() == SIZE,
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
