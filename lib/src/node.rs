use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::context::Context;
use crate::domain::{Contact, NeighborTable, NodeId, Port, RoutingTable, DEFAULT_BUCKET_SIZE};
use crate::messaging::ProtocolMessageSender;
use crate::runtime::Runtime;
use crate::usecases::bootstrap::{BootstrapConfig, BootstrapUseCase};
use crate::usecases::pn_probing::{PNProbingConfig, PNProbingUseCase};
use crate::usecases::{UseCase, UseCaseEvent};

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub bootstrap: BootstrapConfig,
    pub pn_probing: PNProbingConfig,
}

#[derive(Debug)]
pub struct StartError;

impl Display for StartError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node Startup failed")
    }
}

impl Error for StartError {}

#[derive(Debug)]
pub struct HandleMessageError;

impl Display for HandleMessageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Failed to handle Message")
    }
}

impl Error for HandleMessageError {}

/// Users interface to the UseCases.
///
/// Coordinates the interaction of the User with the [UseCase]s.
pub struct Node<C, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    context: C,
    bootstrap: BootstrapUseCase<C, BUCKET_SIZE>,
    pn_probing: PNProbingUseCase<C, BUCKET_SIZE>,
}

impl<C, const BUCKET_SIZE: usize> Node<C, BUCKET_SIZE>
where
    C: Context,
    C::RoutingTable: RoutingTable<BUCKET_SIZE>,
    for<'b> &'b C::RoutingTable: IntoIterator<Item = &'b Contact>,
    C::NeighborTable: NeighborTable,
    for<'b> &'b C::NeighborTable: IntoIterator<Item = (&'b NodeId, &'b Port)>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: Runtime,
{
    pub fn new(config: Config, context: C) -> Self {
        Self {
            context,
            bootstrap: BootstrapUseCase::new(config.bootstrap),
            pn_probing: PNProbingUseCase::new(config.pn_probing),
        }
    }

    pub fn start(&mut self) -> Result<(), StartError> {
        if let Err(e) = self.bootstrap.start(&self.context) {
            log::error!("Failed to start Bootstrap UseCase: {}", e);
            return Err(StartError);
        }

        Ok(())
    }

    pub fn handle_message(&mut self, message: UseCaseEvent) -> Result<(), HandleMessageError> {
        if let Err(e) = self.bootstrap.handle_event(&self.context, message.clone()) {
            log::error!("Bootstrap returned error handling message: {}", e);
            return Err(HandleMessageError);
        }

        if let Err(e) = self.pn_probing.handle_event(&self.context, message) {
            log::error!(
                "Physical Neighbor Probing returned error handling message: {}",
                e
            );
            return Err(HandleMessageError);
        }

        Ok(())
    }
}
