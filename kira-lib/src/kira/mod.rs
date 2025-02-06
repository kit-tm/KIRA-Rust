//! Provides interaction methods with the sans i/o implementation of the routing protocol of KIRA ([R²/KAD](kira_r2kad::R2Kad)).
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
use tokio::sync::mpsc;

use kira_forwarding::tables::{handle_r2kad_request, AsyncNodeIdTable, AsyncPathIdTable};
use kira_r2kad::context::UseCaseContext;
use kira_r2kad::runtime::UseCaseRuntime;
use kira_r2kad::{
    domain::{InsertionStrategy, NodeId, RoutingTable, UNTable, UnderlayNeighborId},
    runtime::R2KadRuntime,
};

pub use kira_forwarding::AsyncForwardingTables;
pub use kira_r2kad::r2kad::R2Kad;
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
/// 2. [forwarding layer](kira_forwarding::AsyncForwardingTables)
/// 3. [underlay observer](crate::underlay).
/// 4. i/o implementation for sending and receiving protocol messages: [io](crate::io).
/// 5. REST-API debug access of the routing daemon: [api](crate::api)
#[derive(Debug)]
pub struct Kira<C, const BUCKET_SIZE: usize, FT> {
    r2kad: R2Kad<C, BUCKET_SIZE>,
    _forwarding_tables: PhantomData<FT>,

    rx_channels: R2KadInputChannels,
    tx_channels: R2KadOutputChannels,
}

impl<C, const BUCKET_SIZE: usize, FT, FTE> Kira<C, BUCKET_SIZE, FT>
where
    FT: AsyncForwardingTables + 'static,
    FT: AsyncNodeIdTable<Error = FTE>,
    FT: AsyncPathIdTable<Error = FTE>,
    FTE: std::error::Error,
    C: UseCaseContext, // root-id for API -- should probably be queried explicitly by API
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
        let (fwtables_tx, mut fwtables_rx) = mpsc::channel(100);
        tokio::task::Builder::new()
            .name("KIRA: Fowarding Tables Channel").spawn(async move {
            loop {
                let Some(req) = fwtables_rx.recv().await else {
                    break;
                };
                if let Err(e) = handle_r2kad_request(&mut forwarding_tables, req).await {
                    log::error!(target: "forwarding_tables", "Handling an ForwardingTablesUpdate failed: {}", e);
                }
            }
        }).unwrap();

        // underlay observer
        let (underlay_tx, underlay_rx) = mpsc::channel(100);
        // adapt stream
        tokio::task::Builder::new()
            .name("Underlay Observer channel")
            .spawn(async move {
                while let Some(update) = underlay_updates.next().await {
                    log::trace!(target: "underlay_observer::update", "Update: {:?}", update);
                    if underlay_tx.send(update).await.is_err() {
                        break;
                    }
                }
            })
            .unwrap();

        // API
        #[allow(unused_variables)]
        let (api_tx, api_rx) = mpsc::channel(100);
        #[cfg(feature = "api")]
        let api_config = api::ApiConfig::new(
            "[::]:8080".parse().unwrap(),
            *r2kad.context().root_id(),
            api_tx,
        );
        #[cfg(feature = "api")]
        tokio::task::Builder::new()
            .name("API Server")
            .spawn(api::start_http_server(api_config))
            .unwrap();

        let (pm_receiver_tx, pm_receiver_rx) = mpsc::channel(100);
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

        let (pm_sender_tx, mut pm_sender_rx) = mpsc::channel(100);
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
            debug: api_rx,
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
    C::UnderlayNeighborTable:
        UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>> + std::fmt::Debug,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + std::fmt::Debug,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::UnderlayNeighborTable, BUCKET_SIZE>,
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
                    log::debug!(target: "kira", "Stopping KIRA as output channel is closed");
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
