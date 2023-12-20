use std::collections::HashMap;
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use crate::context::UseCaseContext;
use crate::domain::{Contact, ContactState, NotVia, RoutingTable};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ProtocolMessageSender, RouteUpdate, UpdateRouteReq};
use crate::use_cases::{ContactEvent, EventHandler, MessageSentFailed, UseCaseEvent};

/// Configuration for contact update handling.
#[derive(Debug)]
pub struct HandleContactUpdateConfig {
    /// Number of overlay neighbors to notify of a change to the routing table.
    pub radius: NonZeroUsize,
    /// Number of bits to group for determining the closeness of contacts
    pub grouping_bits: NonZeroUsize,
}

impl Default for HandleContactUpdateConfig {
    fn default() -> Self {
        HandleContactUpdateConfig {
            radius: NonZeroUsize::new(3).unwrap(),
            grouping_bits: NonZeroUsize::new(1).unwrap(),
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
pub struct HandleContactUpdate<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    config: HandleContactUpdateConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for HandleContactUpdate<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            _pd: PhantomData::default(),
            config: Default::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> HandleContactUpdate<C, BUCKET_SIZE> {
    pub fn new(config: HandleContactUpdateConfig) -> Self {
        Self {
            _pd: PhantomData::default(),
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> HandleContactUpdate<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
{
    fn send_update(
        &self,
        context: &C,
        updates: HashMap<Contact, RouteUpdate>,
    ) -> Result<(), MessageSentFailed> {
        let overlay_neighbors = context
            .routing_table()
            .closest(
                context.root_id(),
                self.config.radius.get(),
                self.config.grouping_bits.get(),
            )
            .expect("type didn't prevent invalid grouping");

        for (_, contact) in overlay_neighbors {
            let message = UpdateRouteReq {
                source_state_seq_nr: *context.pn_table().state_seq_nr(),
                not_via: context.not_via().clone(),
                contact_actions: updates.clone(),
                source_route: SourceRoute::new(context.root_id().clone(), contact.path().clone()),
            };

            if let Err(e) = context.message_sender_mut().send_message(message) {
                log::error!(target: "handle_contact_update", "failed to send UpdateRouteReq: {}", e);
                return Err(MessageSentFailed);
            }
        }

        Ok(())
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
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = MessageSentFailed;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                let mut updates = HashMap::new();
                updates.insert(contact.clone(), RouteUpdate::Removed);

                self.send_update(context, updates)?;
                context.not_via_mut().retain(|not_via| match not_via {
                    NotVia::Link(link) => {
                        link.first() != contact.id() && link.second() != contact.id()
                    }
                });

                self.invalidate_all_affected_contacts(context, &contact);

                if context.pn_table_mut().remove(contact.id()).is_some() {
                    log::debug!(target: "handle_contact_update", "removed {} from physical neighbors", contact.id());
                }
            }
            UseCaseEvent::Contact(ContactEvent::Updated { new, old }) => {
                let mut updates = HashMap::new();
                updates.insert(new.clone(), RouteUpdate::Updated);

                self.send_update(context, updates)?;
                if old.state() == &ContactState::Valid && new.state() != &ContactState::Valid {
                    // Add to not-via data if path gets invalid (maybe done already)
                    self.invalidate_all_affected_contacts(context, &new);
                }
            }
            _ => {}
        }

        Ok(())
    }
}
