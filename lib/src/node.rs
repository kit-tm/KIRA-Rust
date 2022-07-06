use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::context::UseCaseContext;
use crate::domain::{Contact, InsertionStrategy, RoutingTable, DEFAULT_BUCKET_SIZE};
use crate::messaging::ProtocolMessageSender;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::bootstrap::{BootstrapConfig, BootstrapUseCase};
use crate::use_cases::handle_hello::HandleHelloUseCase;
use crate::use_cases::periodic_pn_advertising::{
    PeriodicPNAdvertising, PeriodicPNAdvertisingConfig,
};
use crate::use_cases::random_probing::{RandomProbingConfig, RandomProbingUseCase};
use crate::use_cases::{UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub bootstrap: BootstrapConfig,
    pub pn_probing: PeriodicPNAdvertisingConfig,
    pub random_probing: RandomProbingConfig,
}

#[derive(Debug)]
pub struct StartError;

impl Display for StartError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node Startup failed")
    }
}

impl Error for StartError {}

// TODO: Make more useful
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
    pn_probing: PeriodicPNAdvertising<C, BUCKET_SIZE>,
    random_probing: RandomProbingUseCase<C, BUCKET_SIZE>,
    handle_hello: HandleHelloUseCase<C, BUCKET_SIZE>,
}

impl<C, const BUCKET_SIZE: usize> Node<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::RoutingTable: RoutingTable<BUCKET_SIZE>,
    for<'b> &'b C::RoutingTable: IntoIterator<Item = &'b Contact>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, BUCKET_SIZE>,
{
    pub fn new(config: Config, context: C) -> Self {
        Self {
            context,
            bootstrap: BootstrapUseCase::new(config.bootstrap),
            pn_probing: PeriodicPNAdvertising::new(config.pn_probing),
            random_probing: RandomProbingUseCase::new(config.random_probing),
            handle_hello: HandleHelloUseCase::new(),
        }
    }

    pub fn start(&mut self) -> Result<(), StartError> {
        if let Err(e) = self.bootstrap.start(&self.context) {
            log::error!("Failed to start Bootstrap UseCase: {}", e);
            return Err(StartError);
        }

        if let Err(e) = self.pn_probing.start(&self.context) {
            log::error!("Failed to start PN Probing UseCase: {}", e);
            return Err(StartError);
        }

        if let Err(e) = self.random_probing.start(&self.context) {
            log::error!("Failed to start Random Probing UseCase: {}", e);
            return Err(StartError);
        }

        if let Err(e) = self.handle_hello.start(&self.context) {
            log::error!("Failed to start Hello Message UseCase: {}", e);
            return Err(StartError);
        }

        Ok(())
    }

    pub fn handle_event(&mut self, message: UseCaseEvent) -> Result<(), HandleMessageError> {
        // Delegate Messages to UseCases
        if let Err(e) = self.bootstrap.handle_event(&self.context, message.clone()) {
            log::error!("Bootstrap returned error handling message: {}", e);
            return Err(HandleMessageError);
        }
        if let Err(e) = self.pn_probing.handle_event(&self.context, message.clone()) {
            log::error!(
                "Physical Neighbor Probing returned error handling message: {}",
                e
            );
            return Err(HandleMessageError);
        }
        if let Err(e) = self
            .random_probing
            .handle_event(&self.context, message.clone())
        {
            log::error!("Random Probing returned error handling message: {}", e);
            return Err(HandleMessageError);
        }
        if let Err(e) = self.handle_hello.handle_event(&self.context, message) {
            log::error!("Handling Hello message returned error: {}", e);
            return Err(HandleMessageError);
        }

        // Check States
        let states: Vec<&(dyn UseCaseState)> = vec![
            self.bootstrap.state(),
            self.pn_probing.state(),
            self.random_probing.state(),
            self.handle_hello.state(),
        ];
        if states.iter().any(|use_case| use_case.is_error()) {
            log::error!("Some use case is in error state");
            return Err(HandleMessageError);
        }

        Ok(())
    }
}
