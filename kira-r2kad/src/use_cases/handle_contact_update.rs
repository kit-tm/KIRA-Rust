use std::collections::HashMap;
use std::marker::PhantomData;
use std::num::{NonZeroU8, NonZeroUsize};
use std::ops::Deref;
use tracing::{Level, instrument};

use crate::domain::{
    Contact, ContactState, NodeId, NotVia, RoutingTable, ULNTable, UnderlayNeighborId,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{CommonHeader, ProtocolMessageKind, RouteUpdateActionType, UpdateRouteReq};
use crate::use_cases::{
    ContactEvent, EventHandler, NeverError, UseCaseContext, UseCaseEvent, UseCaseRuntime,
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
                common_header: CommonHeader::new(ProtocolMessageKind::UpdateRouteReq,
                                                 *context.root_id(),
                                                 *contact.id(),
                                                 None,
                                                 Some(From::from(*context.uln_table().state_seq_nr()))),
                not_via: context.not_via().clone(),
                contact_actions: updates.clone(),
                source_route: SourceRoute::new(*context.root_id(), contact.path().clone()),
            };

            context
                .runtime()
                .send_message(message, context.uln_table().deref());
        }
    }

    fn invalidate_all_affected_contacts(&self, context: &C, invalidated_contact: &Contact) {
        // Invalidate all other contacts via this contact
        for mut saved_contact in context.routing_table_mut().iter_mut() {
            if saved_contact.path().starts_with(invalidated_contact.path()) {
                *saved_contact.state_mut() = ContactState::Invalid;
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
        match event {
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                // only send update if underlay neighbor got removed
                // this doesn't happen in reality since we use the UnlimitedULNRoutingTable
                if contact.is_uln() {
                    let mut updates = HashMap::new();
                    updates.insert(contact.clone(), RouteUpdateActionType::WithDraw);

                    log::trace!(target: "handle_contact_update", "Sending update concerning removal of underlay neighbor {}", contact.id());
                    self.send_update(context, updates);
                }

                context.not_via_mut().retain(|not_via| match not_via {
                    NotVia::Link(link) => {
                        link.first() != contact.id() && link.second() != contact.id()
                    }
                });

                self.invalidate_all_affected_contacts(context, &contact);

                if context.uln_table_mut().remove(contact.id()).is_some() {
                    log::trace!(target: "handle_contact_update", "removed {} from underlay neighbors", contact.id());
                }
            }
            UseCaseEvent::Contact(ContactEvent::Updated { new, old }) => {
                let mut updates = HashMap::new();
                if new.state() == &ContactState::Valid {
                    updates.insert(new.clone(), RouteUpdateActionType::Change);
                } else {
                    updates.insert(new.clone(), RouteUpdateActionType::Unreachable);
                }

                // this should be fine, since we only update contacts if interesting anyways
                self.send_update(context, updates);
                if old.state() == &ContactState::Valid && new.state() != &ContactState::Valid {
                    // Add to not-via data if path gets invalid (maybe done already)
                    self.invalidate_all_affected_contacts(context, &new);
                }
            }
            UseCaseEvent::Contact(ContactEvent::New(new)) => {
                // always send an update if a new contact was found
                let mut updates = HashMap::new();
                updates.insert(new.clone(), RouteUpdateActionType::Announce);

                log::trace!(target: "handle_contact_update", "New contact {} found. Sending update.", new.id());
                self.send_update(context, updates);
            }
            _ => {}
        }

        Ok(())
    }
}
