use std::fmt::Debug;
use std::marker::PhantomData;

use crate::context::UseCaseContext;

use super::{EventHandler, MessageSentFailed, UseCaseEvent};

pub struct HandleApi<C> {
    _pd: PhantomData<C>,
}

impl<C> Default for HandleApi<C> {
    fn default() -> Self {
        Self { _pd : PhantomData::default() }
    }
}


impl<C> EventHandler for HandleApi<C>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: Debug,
    C::PhysicalNeighborTable: Debug,
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
            UseCaseEvent::API(super::ApiEvent::RoutingTable(sender)) => {
                sender
                    .send(format!("{:?}", *context.routing_table()))
                    .map_err(|_| MessageSentFailed)?;
            }
            UseCaseEvent::API(super::ApiEvent::PNTable(sender)) => {
                sender
                    .send(format!("{:?}", *context.pn_table()))
                    .map_err(|_| MessageSentFailed)?;
            }
            _ => {}
        }

        Ok(())
    }
}
