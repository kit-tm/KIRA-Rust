use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use core::time::Duration;
use std::marker::PhantomData;

use crate::context::UseCaseContext;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};

use crate::messaging::ProtocolMessageSender;
use crate::use_cases::inject_messages::InjectionResultSender;

// todo what would be a sensible value here?
pub const DEFAULT_PERIODIC_RESTORE: Duration = Duration::from_secs(60 * 60);
pub const DEFAULT_SEND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableInjectorConfig
{
    periodic_restore: Duration,
    send_timeout: Duration
}

impl Default for DistributedHashTableInjectorConfig {
    fn default() -> Self {
        Self {
            periodic_restore: DEFAULT_PERIODIC_RESTORE,
            send_timeout: DEFAULT_SEND_TIMEOUT
        }
    }
}

#[derive(Debug)]
pub enum DHTInjectError {
    SendResultFailed,
    SendFailed,
    Isolated,
}

impl Display for DHTInjectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl Error for DHTInjectError {}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTInjectorState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

pub struct DistributedHashTableInjector<C, IRS>
{
    _c: PhantomData<C>,
    state: DHTInjectorState,
    config: DistributedHashTableInjectorConfig,
    injection_result_sender: IRS,
}

impl UseCaseState for DHTInjectorState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, IRS> DistributedHashTableInjector<C, IRS>
{
    pub fn new(config: DistributedHashTableInjectorConfig, sender: IRS) -> Self {
        Self {
            _c: PhantomData::default(),
            state: DHTInjectorState::default(),
            config,
            injection_result_sender: sender
        }
    }

    pub fn with_default_config(sender: IRS) -> Self {
        Self::new(DistributedHashTableInjectorConfig::default(), sender)
    }
}

impl<C, IRS> DistributedHashTableInjector<C, IRS>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime
{
    fn restore(&mut self) {

    }

    fn start_restore_timer(&mut self, context: &C) {
        let timer_id = context
            .runtime()
            .register_timer(self.config.periodic_restore);

        self.state = DHTInjectorState::Running(timer_id);
    }
}

impl<C, IRS> EventHandler for DistributedHashTableInjector<C, IRS>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        IRS: InjectionResultSender,
{
    type Context = C;
    type Error = DHTInjectError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::InjectMessage(_, _), _) => {
                // react to the injected message we are responsible for
                todo!()
            }
            (UseCaseEvent::Timer(id), DHTInjectorState::Running(our_id)) => {
                if &id == our_id {
                    self.restore();
                    self.start_restore_timer(context);
                }
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, IRS> UseCase for DistributedHashTableInjector<C, IRS>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        IRS: InjectionResultSender,
{
    type State = DHTInjectorState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        self.start_restore_timer(context);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    // todo implement tests
}
