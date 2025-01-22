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

use channels::{R2KadInputChannels, R2KadOutputChannels};
use futures::StreamExt;
use kira_forwarding::tables::handle_r2kad_request;
use kira_forwarding::tables::NodeIdTable;
use kira_forwarding::tables::PathIdTable;
use tokio::sync::mpsc;

pub use kira_forwarding::ForwardingTables;
pub use kira_lib::r2kad::R2Kad;

#[cfg(feature = "api")]
use crate::api;
use crate::underlay;

/// Main KIRA protocol instance.
///
/// The KIRA protocol instance is the orchestrator of the main KIRA components:
///
/// 1. sans i/o implementation of the [R²/KAD](R2Kad) routing protocol
/// 2. [forwarding layer](ForwardingTables)
pub struct Kira<C, const BUCKET_SIZE: usize, FT> {
    pub r2kad: R2Kad<C, BUCKET_SIZE>,
    pub forwarding_tables: FT,

    pub rx_channels: R2KadInputChannels,
    pub tx_channels: R2KadOutputChannels,
}

impl<C, const BUCKET_SIZE: usize, FT, FTE> Kira<C, BUCKET_SIZE, FT>
where
    FT: ForwardingTables + Send + 'static,
    FT: NodeIdTable<Error = FTE>,
    FT: PathIdTable<Error = FTE>,
    FTE: std::error::Error + Send,
{
    pub fn new(r2kad: R2Kad<C, BUCKET_SIZE>, mut forwarding_tables: FT) -> Self {
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
            let (connection, _handle, mut updates) = underlay::observe_underlay()
                .expect("observing underlay using rtnetlink should work");
            tokio::spawn(connection);
            // adapt stream
            tokio::spawn(async move {
                while let Some(update) = updates.next().await {
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

        let rx_channels = R2KadInputChannels {
            api: api_rx,
            protocol_input: todo!("Protocol input"),
            underlay: underlay_rx,
        };

        let tx_channels = R2KadOutputChannels {
            protocol_output: todo!("Protocol output"),
            forwarding: fwtables_tx,
        };

        Self {
            r2kad,
            forwarding_tables,
            rx_channels,
            tx_channels,
        }
    }
}
