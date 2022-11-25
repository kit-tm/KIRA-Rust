//! Type definitions for hardware event handling.

use std::collections::HashSet;

use crate::domain::NetworkInterface;

/// A [HardwareEvent] which represents one or multiple interfaces going up or down.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum HardwareEvent {
    /// One or multiple [NetworkInterface]s have stopped operation.
    InterfacesDown(HashSet<NetworkInterface>),
    /// One or multiple [NetworkInterface]s have started operation.
    InterfacesUp(HashSet<NetworkInterface>),
}

/// A registry used to register handlers of [HardwareEvent]s.
pub trait HardwareEventRegistry {
    /// Registers a new [HardwareEventHandler] to be notified when a [HardwareEvent] occurs.
    fn register_handler<H: 'static + HardwareEventHandler>(&self, handler: H);
}

/// A handler for [HardwareEvent]s.
///
/// Is implemented for all functions that have the signature `fn(HardwareEvent) -> ()` and
/// implement the traits [Send] and [Sync].
pub trait HardwareEventHandler: Send + Sync {
    fn handle(&self, event: HardwareEvent);
}

impl<F> HardwareEventHandler for F
where
    F: Fn(HardwareEvent) + Send + Sync,
{
    fn handle(&self, event: HardwareEvent) {
        self(event);
    }
}
