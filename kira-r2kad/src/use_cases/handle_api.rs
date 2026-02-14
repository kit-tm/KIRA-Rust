use std::fmt::Debug;
use std::marker::PhantomData;
use tracing::{Level, instrument};

use derive_more::derive::{Display, Error};
use tokio::sync::mpsc::error::SendError;

use super::{EventHandler, UseCaseContext, UseCaseEvent, UseCaseRuntime};

#[derive(Debug)]
pub struct HandleApi<C> {
    _pd: PhantomData<C>,
}

impl<C> Default for HandleApi<C> {
    fn default() -> Self {
        Self { _pd: PhantomData }
    }
}

#[derive(Debug, Display, Error)]
#[display("Sending collected API result to caller failed")]
pub struct CallbackChannelError;

impl<T> From<SendError<T>> for CallbackChannelError {
    #[allow(unused_variables)]
    fn from(value: SendError<T>) -> Self {
        Self
    }
}

impl<C> EventHandler for HandleApi<C>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: Debug,
    C::UnderlayNeighborTable: Debug,
{
    type Context = C;

    type Error = CallbackChannelError;

    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "handle_api",
        "handle_api",
        skip(self, context),
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::API(super::ApiEvent::RoutingTable(sender)) => {
                sender.send(format!("{:#?}", *context.routing_table()))?;
            }
            UseCaseEvent::API(super::ApiEvent::ULNTable(sender)) => {
                sender.send(format!("{:#?}", *context.uln_table()))?;
            }
            _ => {}
        }

        Ok(())
    }
}
