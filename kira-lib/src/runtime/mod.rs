//! Interface definition and implementation for use case interaction with a runtime.
//!
use std::collections::HashMap;
use std::ops::Deref;
use std::time::Duration;

use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
use crate::domain::{NodeId, UnderlayNeighborDestination, UnderlayNeighborId};
use crate::messaging::ProtocolMessage;
use crate::use_cases::{BroadcastableUseCaseEvent, TimerId};

#[doc(inline)]
pub use crate::r2kad::runtime::R2KadRuntime;

/// Interface for the [UseCases](crate::use_cases::UseCase) to the runtime environment.
pub trait UseCaseRuntime {
    /// Creates a timer which will later yield a TimerEvent.
    ///
    /// The returned TimerId has to be unique.
    /// It's an error for runtimes to return duplicate [TimerId]s.
    fn register_timer(&mut self, duration: Duration) -> TimerId;

    /// Creates a periodic Timer.
    ///
    /// The returned TimerId has to be unique.
    /// It's an error for runtimes to return duplicate [TimerId]s.
    fn register_periodic_timer(&mut self, duration: Duration) -> TimerId;

    /// Sends a [ProtocolMessage] to a different peer.
    ///
    /// The message will be routed via the underlay neighbor
    /// corresponding to the specified `ulnid`.
    fn send_message_via<P: Into<ProtocolMessage>>(
        &mut self,
        protocol_message: P,
        destination: UnderlayNeighborDestination,
    );

    /// Convenient method to send a [ProtocolMessage].
    ///
    /// If the next hop is not in the `pn_table` (physical neighbor table)
    /// or the even is not source-routed like [HelloMessage](crate::messaging::HelloMessage)
    /// the event is delivered by broadcasting to all interfaces.
    fn send_message<P: Into<ProtocolMessage>>(
        &mut self,
        protocol_message: P,
        pntable: &impl Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    ) {
        let protocol_message: ProtocolMessage = protocol_message.into();
        // WARNING: also broadcasting to neighbors not present in the pntable
        let underlay_dest = protocol_message
            .current_hop()
            .and_then(|next_hop| pntable.get(next_hop))
            .copied()
            .into();

        self.send_message_via(protocol_message, underlay_dest);
    }

    /// Sends an [ForwardingTablesUpdate] request to the forwarding tables.
    fn update_fwd_tables<U: Into<ForwardingTablesUpdate>>(&mut self, update: U);

    /// Broadcast an [UseCaseEvent](BroadcastableUseCaseEvent).
    ///
    /// This event is delivered *locally* to all UseCases.
    fn broadcast_event(&mut self, event: BroadcastableUseCaseEvent);
}
