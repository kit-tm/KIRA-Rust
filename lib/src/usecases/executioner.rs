use std::error::Error;

use crate::usecases::UseCaseEvent;

pub trait Executioner<const ID_SIZE: usize> {
    type Error: Error;

    fn start(self) -> Result<(), Self::Error>;
    fn send_event(&self, event: UseCaseEvent<ID_SIZE>);
}
