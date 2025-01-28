//! Dedicated [Tokio](tokio) synchronization primitives for interaction of
//! the KIRA routing protocol [R²/KAD](kira_lib::R2Kad) and dependent components.
//!
//! The dedicated channels are grouped by [Input](kira_lib::Input) and
//! [Output](kira_lib::Output) event channels:
//!
//! 1. [R2KadInputChannels]: Collection of all types of input channels.
//! 2. [R2KadOutputChannels]: Composed of output
//!
//! # Components
//!
//! The components necessary to provide a complete KIRA implementation based
//! on the routing protocol implementation are:
//!
//! - receiver of [messages](ProtocolMessage) from underlay neighbors
//! - sender of [messages](ProtocolMessage) to underlay neighbors
//! - observer of underlay neighborhood changes
//! - fast forwarding tier (see: [kira_forwarding])
//! - depending on the fast forwarding tier special information providers
//!   of underlay neighbors are needed (typically link-layer information)
//!   but they only interact with the forwarding tier and those are
//!   not represented in channels but type contracts by the forwarding tier.
//!   (see: [kira_forwarding::underlay])

use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{Receiver, Sender};

use kira_forwarding::domain::r2kad::ForwardingTablesUpdate;
use kira_lib::domain::protocol_event::DebugEvent;
use kira_lib::domain::{UnderlayNeighborDestination, UnderlayNeighborId, UnderlayNeighborUpdate};
use kira_lib::messaging::ProtocolMessage;
use kira_lib::{Input, Output};

/// [Receiver] of [ProtocolMessages](ProtocolMessage) and the source [UnderlayNeighborId].
///
/// See [crate::io::receiver] for implementations of message receivers.
/// This [Receiver] is used in the [R2KadInputChannels] to construct [Input::Message] events.
pub type MessageReceiver = Receiver<(ProtocolMessage, UnderlayNeighborId)>;
/// [Receiver] of [ApiEvents](ApiEvent).
///
/// This [Receiver] is used in the [R2KadInputChannels] to construct [DebugEvents](DebugEvent) for [Input::Debug].
pub type DebugReceiver = Receiver<DebugEvent>;
/// [Receiver] of [UnderlayNeighborUpdates](UnderlayNeighborUpdate).
///
/// [UnderlayNeighborUpdates](UnderlayNeighborUpdate) can be obtained using the
/// [UnderlayNeighborUpdatesRx](crate::underlay::UnderlayNeighborUpdatesRx) stream
/// constructed using [crate::underlay::observe_underlay].
/// This [Receiver] is used in the [R2KadInputChannels] to construct [Input::UnderlayUpdate] events.
pub type UnderlayReceiver = Receiver<UnderlayNeighborUpdate>;

/// [Sender] of [ProtocolMessages](ProtocolMessage)  to an [UnderlayNeighborDestination].
///
/// See [crate::io::sender] for implementations of message senders.
/// This [Sender] is used in [R2KadOutputChannels] for processing [Output::SendProtocolMessage] events.
pub type MessageSender = Sender<(ProtocolMessage, UnderlayNeighborDestination)>;
/// [Sender] of [ForwardingTablesUpdates](ForwardingTablesUpdate).
///
/// This [Sender] is used in [R2KadOutputChannels] for processing [Output::UpdateForwardingTables] events.
pub type ForwardingSender = Sender<ForwardingTablesUpdate>;

/// Instance input channels.
#[derive(Debug)]
#[allow(missing_docs)]
pub struct R2KadInputChannels {
    pub debug: DebugReceiver,
    pub protocol_input: MessageReceiver,
    pub underlay: UnderlayReceiver,
}

/// Instance output channels.
#[derive(Debug)]
#[allow(missing_docs)]
pub struct R2KadOutputChannels {
    pub protocol: MessageSender,
    pub forwarding: Option<ForwardingSender>,
}

/// Aggregates all [input channels](R2KadInputChannels) into one [InputSender].
///
/// The [InputSender] can then be used to drive the progress of the routing protocol.
#[tracing::instrument(target = "kira")]
pub(super) async fn input_fan_in(input_channels: &mut R2KadInputChannels) -> Option<Input> {
    let input = tokio::select! {
        biased; // poll in order
        Some((msg, src_ulnid)) = input_channels.protocol_input.recv() => {
            Input::Message(msg, src_ulnid)
        }
        Some(underlay_update) = input_channels.underlay.recv() => {
            Input::UnderlayUpdate(underlay_update)
        }
        Some(api) = input_channels.debug.recv() => {
            Input::Debug(api)
        }
        else => return None,
    };

    Some(input)
}

/// Distributes all [Output] events by an [OutputReceiver] to the dedicated [output
/// channels](R2KadOutputChannels).
///
/// The [OutputReceiver] can be used to listen to output of the routing protocol.
#[tracing::instrument(target = "kira")]
pub(super) async fn output_fan_out(
    output: Output,
    output_channels: &mut R2KadOutputChannels,
) -> Option<()> {
    match output {
        Output::SendProtocolMessage(pm, ulnid) => {
            if output_channels.protocol.send((pm, ulnid)).await.is_err() {
                log::error!(target: "kira", "protocol sending channel closed");
                return None;
            }
        }
        Output::UpdateForwardingTables(update) => {
            let Some(ref forwarding) = output_channels.forwarding else {
                log::trace!(target: "kira", "Ignoring forwarding tables update: {}", update);
                return Some(());
            };
            if let Err(SendError(update)) = forwarding.send(update).await {
                log::warn!(
                    target: "kira",
                    "forwarding tables update can't be delivered because channel is closed: {}",
                    update
                );
                let _ = output_channels.forwarding.take();
            }
        }
    }
    Some(())
}
