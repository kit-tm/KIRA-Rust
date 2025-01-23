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

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use kira_forwarding::domain::r2kad::ForwardingTablesUpdate;
use kira_lib::domain::protocol_event::DebugEvent;
use kira_lib::domain::{UnderlayNeighborDestination, UnderlayNeighborId, UnderlayNeighborUpdate};
use kira_lib::messaging::ProtocolMessage;
use kira_lib::use_cases::ApiEvent;
use kira_lib::{Input, Output};

pub type InputSender = UnboundedSender<Input>;
pub type MessageReceiver = Receiver<(ProtocolMessage, UnderlayNeighborId)>;
pub type ApiReceiver = Receiver<ApiEvent>;
pub type UnderlayReceiver = Receiver<UnderlayNeighborUpdate>;

pub type OutputReceiver = UnboundedReceiver<Output>;
pub type MessageSender = Sender<(ProtocolMessage, UnderlayNeighborDestination)>;
pub type ForwardingSender = Sender<ForwardingTablesUpdate>;

pub trait ProtocolInstance {}

/// Instance input channels.
pub struct R2KadInputChannels {
    /// Events receiver from the management component.
    pub api: ApiReceiver,
    /// Deserialized [ProtocolMessages](ProtocolMessage) received from other underlay neighbors.
    pub protocol_input: MessageReceiver,
    /// Notifications of changes in the underlay neighborhood of the node.
    pub underlay: UnderlayReceiver,
}

/// Instance output channels.
pub struct R2KadOutputChannels {
    pub protocol_output: MessageSender,
    pub forwarding: ForwardingSender,
}

/// Aggregates all [input channels](R2KadInputChannels) into one [InputSender].
///
/// The [InputSender] can then be used to drive the progress of the routing protocol.
pub(super) fn input_fan_in(
    input_channels: R2KadInputChannels,
    fan_in: InputSender,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut input_channels = input_channels;
        loop {
            let input = tokio::select! {
                Some(api) = input_channels.api.recv() => {
                    Input::Debug(DebugEvent::Api(api))
                }
                Some((msg, src_ulnid)) = input_channels.protocol_input.recv() => {
                    Input::Message(msg, src_ulnid)
                }
                Some(underlay_update) = input_channels.underlay.recv() => {
                    Input::UnderlayUpdate(underlay_update)
                }
            };

            if fan_in.send(input).is_err() {
                log::warn!("Fan in of input events aborted because receiver closed");
                break;
            }
        }
    })
}

/// Distributes all [Output] events by an [OutputReceiver] to the dedicated [output
/// channels](R2KadOutputChannels).
///
/// The [OutputReceiver] can be used to listen to output of the routing protocol.
pub(super) fn output_fan_out(
    mut combined_output: OutputReceiver,
    output_channels: R2KadOutputChannels,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(output) = combined_output.recv().await {
            match output {
                Output::SendProtocolMessage(pm, ulnid) => {
                    let _ = output_channels.protocol_output.send((pm, ulnid)).await;
                }
                Output::UpdateForwardingTables(fwtables_update) => {
                    let _ = output_channels.forwarding.send(fwtables_update).await;
                }
            }
        }
    })
}
