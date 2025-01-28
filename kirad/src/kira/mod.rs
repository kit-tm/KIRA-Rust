//! Provides interaction methods with the sans i/o implementation of the routing protocol of KIRA ([R²/KAD](kira_forwarding::R2Kad)).
//!
//! # Channels
//!
//! The [channels] module provide dedicated [Tokio](tokio) powered channels to
//! provide async communication in-between the different components
//! required by the routing protocol.
//!
//! For further information see the module documentation of [channels].

pub mod channels;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::Instant;

use channels::{R2KadInputChannels, R2KadOutputChannels};
use futures::StreamExt;
use kira_forwarding::tables::handle_r2kad_request;
use kira_forwarding::tables::NodeIdTable;
use kira_forwarding::tables::PathIdTable;
use kira_lib::context::UseCaseContext;
use kira_lib::domain::InsertionStrategy;
use kira_lib::domain::NodeId;
use kira_lib::domain::RoutingTable;
use kira_lib::domain::UNTable;
use kira_lib::domain::UnderlayNeighborId;
use kira_lib::runtime::R2KadRuntime;
use kira_lib::runtime::UseCaseRuntime;
use tokio::sync::mpsc;

pub use kira_forwarding::ForwardingTables;
pub use kira_lib::r2kad::R2Kad;
use tokio::task::yield_now;
use tokio::time;

#[cfg(feature = "api")]
use crate::api;
use crate::io::receiver::error::RecvError;
use crate::io::receiver::AsyncProtocolMessageReceiver;
use crate::io::sender::error::SenderError;
use crate::io::sender::AsyncProtocolMessageSender;
use crate::underlay::UnderlayNeighborUpdatesRx;

/// Main KIRA protocol instance.
///
/// The KIRA protocol instance is the orchestrator of the main KIRA components:
///
/// 1. sans i/o implementation of the [R²/KAD](R2Kad) routing protocol
/// 2. [forwarding layer](ForwardingTables)
#[derive(Debug)]
pub struct Kira<C, const BUCKET_SIZE: usize, FT> {
    r2kad: R2Kad<C, BUCKET_SIZE>,
    _forwarding_tables: PhantomData<FT>,

    rx_channels: R2KadInputChannels,
    tx_channels: R2KadOutputChannels,
}

impl<C, const BUCKET_SIZE: usize, FT, FTE> Kira<C, BUCKET_SIZE, FT>
where
    FT: ForwardingTables + Send + 'static,
    FT: NodeIdTable<Error = FTE>,
    FT: PathIdTable<Error = FTE>,
    FTE: std::error::Error + Send,
{
    /// Create fully functional [Kira] instance.
    ///
    /// It sets up various [tasks][tokio::task] for driving progress
    /// of the individual components (fast forwarding table, R²/KAD instance, ...)
    /// individually in an async manner.
    /// This method needs to be spawned inside a [tokio] runtime or it will panic.
    ///
    /// If you want to connect custom components for debugging use [Self::with_channels]
    pub fn with_components(
        r2kad: R2Kad<C, BUCKET_SIZE>,
        mut forwarding_tables: FT,
        mut underlay_updates: UnderlayNeighborUpdatesRx,
        mut pm_receiver: impl AsyncProtocolMessageReceiver + std::fmt::Debug + 'static,
        mut pm_sender: impl AsyncProtocolMessageSender + std::fmt::Debug + 'static,
    ) -> Self {
        // forwarding tables
        let (fwtables_tx, mut fwtables_rx) = mpsc::channel(10);
        tokio::task::Builder::new()
            .name("KIRA: Fowarding Tables Channel").spawn(async move {
            loop {
                let Some(req) = fwtables_rx.recv().await else {
                    break;
                };
                if let Err(e) = handle_r2kad_request(&mut forwarding_tables, req) {
                    log::error!(target: "forwarding_tables", "Handling an ForwardingTablesUpdate failed: {}", e);
                }
            }
        }).unwrap();

        // underlay observer
        let (underlay_tx, underlay_rx) = mpsc::channel(10);
        // adapt stream
        tokio::task::Builder::new()
            .name("Underlay Observer channel")
            .spawn(async move {
                while let Some(update) = underlay_updates.next().await {
                    underlay_tx.send(update).await.unwrap();
                }
            })
            .unwrap();

        // TODO: shutdown on signal

        // API
        #[allow(unused_variables)]
        let (api_tx, api_rx) = mpsc::channel(10);
        #[cfg(feature = "api")]
        let api_config = api::ApiConfig::new(
            "[::]:8080".parse().unwrap(),
            todo!(target: "kira", "root_id for API"),
            api_tx,
        );
        #[cfg(feature = "api")]
        tokio::spawn(api::start_http_server(api_config));

        let (pm_receiver_tx, pm_receiver_rx) = mpsc::channel(10);
        tokio::task::Builder::new().name("KIRA: Protocol Message Receiver channel").spawn(async move {
            while let Some(recv) = pm_receiver.recv().await {
                match recv {
                    Ok(msg) => {
                        let _ = pm_receiver_tx.send(msg).await;
                    }
                    Err(RecvError::Closed) => {
                        log::debug!(target: "kira", "Protocol message receiver closed");
                        break;
                    }
                    Err(other_error) => {
                        log::error!(target: "kira", "Error on receiving protocol messages: {}", other_error)
                    }
                }
            }
            log::debug!(target: "kira", "Protocol message receiver finished: {pm_receiver:?}");
        }).unwrap();

        let (pm_sender_tx, mut pm_sender_rx) = mpsc::channel(10);
        tokio::task::Builder::new().name("KIRA: Protocol Message Sender channel").spawn(async move {
            while let Some((msg, dest)) = pm_sender_rx.recv().await {
                match pm_sender.send_message(msg, dest).await {
                    Ok(()) => {}
                    Err(SenderError::Closed) => {
                        log::debug!(target: "kira", "Protocol message sender closed: {}", SenderError::Closed);
                        break;
                    }
                    Err(other_error) => {
                        log::error!(target: "kira", "Sending protocol message failed: {}", other_error)
                    }
                }
            }
            log::warn!(target: "kira", "Channel for sending protocol messages closed");
        }).unwrap();

        let rx_channels = R2KadInputChannels {
            api: api_rx,
            protocol_input: pm_receiver_rx,
            underlay: underlay_rx,
        };

        let tx_channels = R2KadOutputChannels {
            protocol: pm_sender_tx,
            forwarding: Some(fwtables_tx),
        };

        Self::with_channels(r2kad, rx_channels, tx_channels)
    }
}
impl<C, const BUCKET_SIZE: usize, FT> Kira<C, BUCKET_SIZE, FT> {
    /// Create a new [Kira] instance.
    ///
    /// This instance can be started using [Kira::start].
    pub fn with_channels(
        r2kad: R2Kad<C, BUCKET_SIZE>,
        rx_channels: R2KadInputChannels,
        tx_channels: R2KadOutputChannels,
    ) -> Self {
        Self {
            r2kad,
            _forwarding_tables: Default::default(),
            rx_channels,
            tx_channels,
        }
    }
}

impl<C, const BUCKET_SIZE: usize, FT> Kira<C, BUCKET_SIZE, FT>
where
    C: UseCaseContext,
    C::Runtime: Deref<Target = R2KadRuntime>,
    C::Runtime: UseCaseRuntime,
    C::PhysicalNeighborTable:
        UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>> + std::fmt::Debug,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + std::fmt::Debug,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::PhysicalNeighborTable, BUCKET_SIZE>,
{
    /// Starts the R²/KAD routing protocol instance.
    #[tracing::instrument(target = "kira", skip_all)]
    pub async fn start(mut self) {
        let Self {
            ref mut r2kad,
            ref mut rx_channels,
            ref mut tx_channels,
            ..
        } = self;

        log::trace!(target: "kira",  "Startup protocol instance");
        {
            let now = Instant::now();
            if let Err(e) = r2kad.startup(now) {
                log::error!(target: "kira", "Error on startup of R²/KAD: {}", e);
                return;
            }
        }

        loop {
            // process output firstly to capture startup output
            while let Some(output) = r2kad.poll_output() {
                if channels::output_fan_out(output, tx_channels)
                    .await
                    .is_none()
                {
                    log::error!(target: "kira", "output channels closed");
                    return;
                }
            }

            yield_now().await;

            // handle the rare case that the protocol instance does not utilize a single timer
            let Some(timer_due) = r2kad.poll_timeout() else {
                log::warn!(target: "kira", "R²/KAD has no timeout");
                log::trace!(target: "kira", "Waiting for new input events");

                // only process Input events then
                let Some(input) = channels::input_fan_in(rx_channels).await else {
                    log::info!(target: "kira", "Fan in channel closed and no timers left");
                    return;
                };
                log::trace!(target: "kira", "Process input: {:?}", input);
                let now = Instant::now();
                if let Err(e) = r2kad.handle_input(input, now) {
                    log::error!(target: "kira", "Error handle_timeout: {}", e);
                    return;
                }

                continue;
            };

            //time::sleep(Duration::from_secs(2)).await;
            log::trace!(target: "kira", "Waiting for new input events or timeout ({:?})", timer_due);
            tokio::select! {
                biased; // poll in order since we check timers on handling input regardlessly

                Some(input) = channels::input_fan_in(rx_channels) => {
                    log::trace!(target: "kira", "Process input: {:?}", input);
                    let now = Instant::now();
                    if let Err(e) = r2kad.handle_input(input, now) {
                        log::error!(target: "kira", "Error handle_input: {}", e);
                        return;
                    }
                }
                _ = time::sleep_until(timer_due.into()) => {
                    log::trace!(target: "kira", "Process timeout");
                    let now = Instant::now();
                    if let Err(e) = r2kad.handle_timeout(now) {
                        log::error!(target: "kira", "Error handle_timeout: {}", e);
                        return;
                    }
                }
                // no else required because timer MUST complete
            }
        }
    }
}
