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

use futures::Stream;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{Receiver, Sender};

use kira_forwarding::domain::r2kad::ForwardingTablesUpdate;
use kira_lib::domain::protocol_event::{forwarding, DebugEvent};
use kira_lib::domain::{UnderlayNeighborDestination, UnderlayNeighborId, UnderlayNeighborUpdate};
use kira_lib::messaging::ProtocolMessage;
use kira_lib::use_cases::ApiEvent;
use kira_lib::{Input, Output};

pub type MessageReceiver = Receiver<(ProtocolMessage, UnderlayNeighborId)>;
pub type ApiReceiver = Receiver<ApiEvent>;
pub type UnderlayReceiver = Receiver<UnderlayNeighborUpdate>;

pub type MessageSender = Sender<(ProtocolMessage, UnderlayNeighborDestination)>;
pub type ForwardingSender = Sender<ForwardingTablesUpdate>;

pub trait ProtocolInstance {}

/// Instance input channels.
#[derive(Debug)]
pub struct R2KadInputChannels {
    /// Events receiver from the management component.
    pub api: ApiReceiver,
    /// Deserialized [ProtocolMessages](ProtocolMessage) received from other underlay neighbors.
    pub protocol_input: MessageReceiver,
    /// Notifications of changes in the underlay neighborhood of the node.
    pub underlay: UnderlayReceiver,
}

/// Instance output channels.
#[derive(Debug)]
pub struct R2KadOutputChannels {
    pub protocol: MessageSender,
    pub forwarding: Option<ForwardingSender>,
}

/// Aggregates all [input channels](R2KadInputChannels) into one [InputSender].
///
/// The [InputSender] can then be used to drive the progress of the routing protocol.
pub(super) async fn input_fan_in(input_channels: &mut R2KadInputChannels) -> Option<Input> {
    let input = tokio::select! {
        biased; // poll in order
        Some((msg, src_ulnid)) = input_channels.protocol_input.recv() => {
            Input::Message(msg, src_ulnid)
        }
        Some(underlay_update) = input_channels.underlay.recv() => {
            Input::UnderlayUpdate(underlay_update)
        }
        Some(api) = input_channels.api.recv() => {
            Input::Debug(DebugEvent::Api(api))
        }
        else => return None,
    };

    Some(input)
}

/// Distributes all [Output] events by an [OutputReceiver] to the dedicated [output
/// channels](R2KadOutputChannels).
///
/// The [OutputReceiver] can be used to listen to output of the routing protocol.
pub(super) async fn output_fan_out(
    output: Output,
    output_channels: &mut R2KadOutputChannels,
) -> Option<()> {
    match output {
        Output::SendProtocolMessage(pm, ulnid) => {
            if output_channels.protocol.send((pm, ulnid)).await.is_err() {
                log::error!("protocol sending channel closed");
                return None;
            }
        }
        Output::UpdateForwardingTables(update) => {
            let Some(ref forwarding) = output_channels.forwarding else {
                log::trace!("Ignoring forwarding tables update: {}", update);
                return Some(());
            };
            if let Err(SendError(update)) = forwarding.send(update).await {
                log::warn!(
                    "forwarding tables update can't be delivered because channel is closed: {}",
                    update
                );
                let _ = output_channels.forwarding.take();
            }
        }
    }
    Some(())
}
