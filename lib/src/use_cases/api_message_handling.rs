use std::marker::PhantomData;
use tokio::sync::mpsc::error::SendError;
use crate::context::UseCaseContext;
use crate::domain::api::{RoutingTable};
use crate::use_cases::{ApiEvent, EventHandler, UseCaseEvent};

pub struct HandleApiMessages<C, const BUCKET_SIZE: usize> {
    context_type: PhantomData<C>
}

impl<C, const BUCKET_SIZE: usize> Default for HandleApiMessages<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            context_type: PhantomData::default()
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for HandleApiMessages<C, BUCKET_SIZE>
where
    C : UseCaseContext,
    for<'a> C::RoutingTable: crate::domain::RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = SendError<RoutingTable>;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::API(ApiEvent::RoutingTable(tx)) => {
                tx.send(context.to_api_model())?;
                Ok(())
            }
            _ => Ok(())
        }
    }

}
