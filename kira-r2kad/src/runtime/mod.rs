//! Interface definition and implementation for use case interaction with a runtime.

use std::collections::HashMap;
use std::ops::Deref;
use std::time::{Duration, Instant};

use rand::Rng;

use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
use crate::domain::{NodeId, UnderlayNeighborDestination, UnderlayNeighborId};
use crate::messaging::ProtocolMessage;
use crate::use_cases::{BroadcastableUseCaseEvent, TimerId};

pub use crate::r2kad::runtime::R2KadRuntime;

#[cfg(test)]
pub mod testing;

/// Interface for the [UseCases](crate::use_cases::UseCase) to the runtime environment.
pub trait UseCaseRuntime {
    /// Creates a timer which will later yield a TimerEvent.
    ///
    /// The returned TimerId has to be unique.
    /// It's an error for runtimes to return duplicate [TimerId]s.
    fn register_timer(&self, duration: Duration) -> TimerId;

    /// Creates a periodic Timer.
    ///
    /// The returned TimerId has to be unique.
    /// It's an error for runtimes to return duplicate [TimerId]s.
    fn register_periodic_timer(&self, duration: Duration) -> TimerId;

    fn register_rand_timer(&self, duration: Duration) -> TimerId {
        let mut rng = rand::rng();
        let factor = rng.random_range(0.5..=1.5);
        self.register_timer(duration.mul_f64(factor))
    }

    /// Get the [Duration] left of a timer if one is known by the [TimerId].
    fn timer_remaining_duration(&self, timer: &TimerId) -> Option<Duration>;

    /// Get the current time.
    fn current_time(&self) -> Instant;

    /// removes a pending timer if it exists,
    fn remove_timer(&self, timer_id: TimerId);

    /// Sends a [ProtocolMessage] to a different peer.
    ///
    /// The message will be routed via the underlay neighbor
    /// corresponding to the specified `ulnid`.
    fn send_message_via<P: Into<ProtocolMessage>>(
        &self,
        protocol_message: P,
        destination: UnderlayNeighborDestination,
    );

    /// Convenient method to send a [ProtocolMessage].
    ///
    /// If the next hop is not in the `uln_table` (underlay neighbor table)
    /// or the even is source-routed like [ULNHello messages](crate::messaging::ProtocolMessage::ULNHello)
    /// a warning is logged
    fn send_message<P: Into<ProtocolMessage>>(
        &self,
        protocol_message: P,
        ulntable: &impl Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    ) {
        let protocol_message: ProtocolMessage = protocol_message.into();
        let underlay_dest = if let Some(next_hop) = protocol_message.current_hop() {
            let Some(uln_dest) = ulntable.get(next_hop) else {
                tracing::warn!(
                    %next_hop,
                    reason = "uln_dest of next hop unknown",
                    ?protocol_message,
                    "Dropping message"
                );
                return;
            };
            (*uln_dest).into()
        } else {
            UnderlayNeighborDestination::Broadcast
        };

        self.send_message_via(protocol_message, underlay_dest);
    }

    /// Sends an [ForwardingTablesUpdate] request to the forwarding tables.
    fn update_fwd_tables<U: Into<ForwardingTablesUpdate>>(&self, update: U);

    /// Broadcast an [UseCaseEvent](BroadcastableUseCaseEvent).
    ///
    /// This event is delivered *locally* to all UseCases.
    fn broadcast_event<B: Into<BroadcastableUseCaseEvent>>(&self, event: B);
}

impl<UR: UseCaseRuntime, D: Deref<Target = UR>> UseCaseRuntime for D {
    fn register_timer(&self, duration: Duration) -> TimerId {
        self.deref().register_timer(duration)
    }

    fn register_periodic_timer(&self, duration: Duration) -> TimerId {
        self.deref().register_periodic_timer(duration)
    }

    fn timer_remaining_duration(&self, timer: &TimerId) -> Option<Duration> {
        self.deref().timer_remaining_duration(timer)
    }

    fn current_time(&self) -> Instant {
        self.deref().current_time()
    }

    fn remove_timer(&self, timer_id: TimerId) {
        self.deref().remove_timer(timer_id)
    }
    fn send_message_via<P: Into<ProtocolMessage>>(
        &self,
        protocol_message: P,
        destination: UnderlayNeighborDestination,
    ) {
        self.deref().send_message_via(protocol_message, destination);
    }

    fn update_fwd_tables<U: Into<ForwardingTablesUpdate>>(&self, update: U) {
        self.deref().update_fwd_tables(update);
    }

    fn broadcast_event<B: Into<BroadcastableUseCaseEvent>>(&self, event: B) {
        self.deref().broadcast_event(event);
    }
}
