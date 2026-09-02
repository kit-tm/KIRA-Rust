use std::{
    collections::HashMap,
    marker::PhantomData,
    num::{
        NonZeroU8,
        NonZeroUsize,
    },
    ops::Deref,
};

use tracing::{
    Level,
    instrument,
};

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
        ULNTable,
        UnderlayNeighborId,
        protocol_message::{
            CommonHeader,
            RouteUpdateActionType,
            UpdateRouteReq,
        },
    },
    use_cases::{
        ContactEvent,
        EventHandler,
        NeverError,
        UseCaseContext,
        UseCaseEvent,
        UseCaseRuntime,
    },
};

/// Configuration for contact update handling.
#[derive(Debug)]
pub struct HandleContactUpdateConfig {
    /// Number of overlay neighbors to notify of a change to the routing table.
    pub radius: NonZeroUsize,
    /// Number of bits to group for determining the closeness of contacts
    pub grouping_bits: NonZeroU8,
}

impl Default for HandleContactUpdateConfig {
    fn default() -> Self {
        HandleContactUpdateConfig {
            radius: NonZeroUsize::new(3).unwrap(),
            grouping_bits: NonZeroU8::MIN,
        }
    }
}

/// EventHandler for the common behavior to send UpdateRouteReq if a contacts path gets updated.
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
}

impl<C, const BUCKET_SIZE: usize> Default for HandleContactUpdate<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            _pd: PhantomData,
            config: Default::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> HandleContactUpdate<C, BUCKET_SIZE> {
    pub fn new(config: HandleContactUpdateConfig) -> Self {
        Self {
            _pd: PhantomData,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> HandleContactUpdate<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn send_update(&self, context: &C, updates: HashMap<Contact, RouteUpdateActionType>) {
        let overlay_neighbors = context
            .routing_table()
            .closest(
                context.root_id(),
                self.config.radius.get(),
                self.config.grouping_bits,
            )
            .expect("type didn't prevent invalid grouping");

        for (_, contact) in overlay_neighbors {
            let message = UpdateRouteReq {
                common_header: CommonHeader::new(
                    ProtocolMessageKind::UpdateRouteReq,
                    *context.root_id(),
                    *contact.id(),
                    None,
                    *context.uln_table().state_seq_nr(),
                    context.uln_table().size(),
                ),
                not_via: None,
                contact_actions: updates.clone(),
                source_route: SourceRoute::new(*context.root_id(), contact.path().unwrap().clone()),
            };

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
                if let ContactState::Invalid(not_via_state_list) = invalidated_contact.state() {
                    // use not via info from invalidated contact
                    saved_contact.set_invalid(not_via_state_list.clone());
                } else {
                    let nvs = NotViaState::new(
                        Link::new(*context.root_id(), *invalidated_contact.id()),
                        Timestamp::now(),
                    );
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
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = NeverError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "handle_contact_update",
        "handle_contact_update",
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
        if let UseCaseEvent::Contact(contact_event) = event {
            // handle contact events
            match *contact_event {
                ContactEvent::Removed(contact) => {
                    // only send update if underlay neighbor got removed
                    // this doesn't happen in reality since we use the UnlimitedULNRoutingTable
                    if contact.is_uln() {
                        let mut updates = HashMap::new();
                        updates.insert(contact.clone(), RouteUpdateActionType::WithDraw);

                        log::trace!(target: "handle_contact_update", "Sending update concerning removal of underlay neighbor {}", contact.id());
                        self.send_update(context, updates);
                    }

                    self.invalidate_all_affected_contacts(context, &contact);

                    if context.uln_table_mut().remove(contact.id()).is_some() {
                        log::trace!(target: "handle_contact_update", "removed {} from underlay neighbors", contact.id());
                    }
                }
                ContactEvent::Updated { new, old } => {
                    let mut updates = HashMap::new();
                    if new.state() == &ContactState::Valid {
                        updates.insert(*new.clone(), RouteUpdateActionType::Change);
                    } else {
                        updates.insert(*new.clone(), RouteUpdateActionType::Unreachable);
                    }

                    // this should be fine, since we only update contacts if interesting anyways
                    self.send_update(context, updates);
                    if old.is_valid() && !new.is_valid() {
                        // Add to not-via data if path gets invalid (maybe done already)
                        self.invalidate_all_affected_contacts(context, &new);
                    }
                }
                ContactEvent::New(new) => {
                    // always send an update if a new contact was found
                    let mut updates = HashMap::new();
                    updates.insert(new.clone(), RouteUpdateActionType::Announce);

                    log::trace!(target: "handle_contact_update", "New contact {} found. Sending update.", new.id());
                    self.send_update(context, updates);
                }
                _ => {}
            }
        }

        Ok(())
    }
}
