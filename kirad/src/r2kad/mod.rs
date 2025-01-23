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
use std::future::Future;
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
use tokio::time;

#[cfg(feature = "api")]
use crate::api;
use crate::io::receiver::error::RecvError;
use crate::io::receiver::AsyncProtocolMessageReceiver;
use crate::io::sender::error::SenderError;
use crate::io::sender::AsyncProtocolMessageSender;
use crate::underlay;
use crate::underlay::UnderlayNeighborUpdatesRx;

/// Main KIRA protocol instance.
///
/// The KIRA protocol instance is the orchestrator of the main KIRA components:
///
/// 1. sans i/o implementation of the [R²/KAD](R2Kad) routing protocol
/// 2. [forwarding layer](ForwardingTables)
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
    #![allow(missing_docs)]

    pub fn new(
        r2kad: R2Kad<C, BUCKET_SIZE>,
        mut forwarding_tables: FT,
        mut underlay_updates: UnderlayNeighborUpdatesRx,
        mut pm_receiver: impl AsyncProtocolMessageReceiver + std::fmt::Debug + 'static,
        mut pm_sender: impl AsyncProtocolMessageSender + std::fmt::Debug + 'static,
    ) -> Self {
        // forwarding tables
        let (fwtables_tx, mut fwtables_rx) = mpsc::channel(10);
        tokio::task::spawn(async move {
            loop {
                let Some(req) = fwtables_rx.recv().await else {
                    break;
                };
                if let Err(e) = handle_r2kad_request(&mut forwarding_tables, req) {
                    log::error!(target: "forwarding_tables", "Handling an ForwardingTablesUpdate failed: {}", e);
                }
            }
        });

        // underlay observer
        let (underlay_tx, underlay_rx) = mpsc::channel(10);
        {
            // adapt stream
            tokio::spawn(async move {
                while let Some(update) = underlay_updates.next().await {
                    underlay_tx.send(update).await.unwrap();
                }
            });
        }

        // TODO: connect handle with message receiver
        // TODO: message receiver
        // TODO: message sender
        // TODO: shutdown on signal

        // API
        #[allow(unused_variables)]
        let (api_tx, api_rx) = mpsc::channel(10);
        #[cfg(feature = "api")]
        let api_config = api::ApiConfig::new(
            "[::]:8080".parse().unwrap(),
            todo!("root_id for API"),
            api_tx,
        );
        #[cfg(feature = "api")]
        tokio::spawn(api::start_http_server(api_config));

        let (pm_receiver_tx, pm_receiver_rx) = mpsc::channel(10);
        tokio::spawn(async move {
            loop {
                match pm_receiver.recv().await {
                    Ok(None) => {
                        log::debug!("Protocol message receiver finished: {pm_receiver:?}");
                        break;
                    }
                    Ok(Some(msg)) => {
                        let _ = pm_receiver_tx.send(msg).await;
                    }
                    Err(RecvError::Closed(e)) => {
                        log::debug!("Protocol message receiver closed: {}", RecvError::Closed(e));
                        break;
                    }
                    Err(other_error) => {
                        log::error!("Error on receiving protocol messages: {}", other_error)
                    }
                }
            }
        });

        let (pm_sender_tx, mut pm_sender_rx) = mpsc::channel(10);
        tokio::spawn(async move {
            loop {
                match pm_sender_rx.recv().await {
                    Some((msg, dest)) => match pm_sender.send_message(msg, dest).await {
                        Ok(()) => {}
                        Err(SenderError::Closed) => {
                            log::debug!("Protocol message sender closed: {}", SenderError::Closed);
                            break;
                        }
                        Err(other_error) => {
                            log::error!("Error on sending protocol message: {}", other_error)
                        }
                    },
                    None => {
                        log::trace!("Channel for sending protocol messages closed");
                        break;
                    }
                }
            }
        });

        let rx_channels = R2KadInputChannels {
            api: api_rx,
            protocol_input: pm_receiver_rx,
            underlay: underlay_rx,
        };

        let tx_channels = R2KadOutputChannels {
            protocol_output: pm_sender_tx,
            forwarding: fwtables_tx,
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
    C: UseCaseContext<Runtime = R2KadRuntime>,
    C::Runtime: UseCaseRuntime,
    C::PhysicalNeighborTable:
        UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>> + std::fmt::Debug,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + std::fmt::Debug,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::PhysicalNeighborTable, BUCKET_SIZE>,
{
    /// Starts the R²/KAD routing protocol instance.
    pub fn start(self) -> impl Future {
        // FIXME: unbound_channel/channel does not make sense like this for creating back pressure
        let (input_tx, mut input_rx) = mpsc::unbounded_channel();
        let (output_tx, output_rx) = mpsc::unbounded_channel();

        async move {
            let fan_in = channels::input_fan_in(self.rx_channels, input_tx);
            let fan_out = channels::output_fan_out(output_rx, self.tx_channels);

            let mut r2kad = self.r2kad;

            log::trace!("Startup R²/KAD protocol instance");
            let now = Instant::now();
            if let Err(e) = r2kad.startup(now) {
                log::error!("Error on startup of R²/KAD: {}", e);
                return;
            }

            // MAIN EVENT LOOP
            let mut timer_due = r2kad.poll_timeout();
            loop {
                if let Some(timer_due) = timer_due {
                    log::trace!("Waiting for new input events or timeout");
                    tokio::select! {
                        _ = time::sleep_until(timer_due.into()) => {
                            let now = Instant::now();
                            if let Err(e) = r2kad.handle_timeout(now) {
                                log::error!("Error handle_timeout: {}", e);
                                break;
                            }
                        }
                        Some(input) = input_rx.recv() => {
                            let now = Instant::now();
                            if let Err(e) = r2kad.handle_input(input, now) {
                                log::error!("Error handle_input: {}", e);
                                break;
                            }

                        }
                        else => { continue}
                    }
                } else {
                    log::trace!("Waiting for new input events");
                    match input_rx.recv().await {
                        Some(input) => {
                            let now = Instant::now();
                            if let Err(e) = r2kad.handle_input(input, now) {
                                log::error!("Error handle_timeout: {}", e);
                                break;
                            }
                        }
                        None => {
                            log::info!("Fan in channel closed");
                            break;
                        }
                    }
                }

                log::trace!("Process R²/KAD output");

                while let Some(output) = r2kad.poll_output() {
                    if output_tx.send(output).is_err() {
                        log::error!("Output channel closed");
                        break;
                    }
                }

                timer_due = r2kad.poll_timeout();
            }

            // drop channels
            let _ = output_tx;
            let _ = input_rx;

            log::info!("Waiting for input and output channels to shutdown");

            let (fan_in, fan_out) = tokio::join!(fan_in, fan_out);
            if let Err(e) = fan_in {
                log::warn!("Error waiting on fan_in: {}", e);
            }
            if let Err(e) = fan_out {
                log::warn!("Error waiting on fan_out: {}", e);
            }

            log::trace!("finished");
        }
    }
}
