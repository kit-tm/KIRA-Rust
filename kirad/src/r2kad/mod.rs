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

use kira_forwarding::tables::handle_r2kad_request;
use kira_forwarding::tables::NodeIdTable;
pub use kira_forwarding::ForwardingTables;
pub use kira_lib::r2kad::R2Kad;
use tokio::sync::mpsc;

use crate::node::channels::NodeChannelsRx;

/// Main KIRA protocol instance.
///
/// The KIRA protocol instance is the orchestrator for the main KIRA components:
///
/// 1. sans i/o implementation of the [R²/KAD](R2Kad) routing protocol
/// 2. [forwarding layer](ForwardingTables)
pub struct Kira<C, const BUCKET_SIZE: usize, FT> {
    pub r2kad: R2Kad<C, BUCKET_SIZE>,
    pub forwarding_tables: FT,

    pub rx_channels: NodeChannelsRx<Self>,
    pub tx_channels: NodeChannelsTx<Self>,
}

impl<C, const BUCKET_SIZE: usize, FT, FTE> Kira<C, BUCKET_SIZE, FT>
where
    FT: ForwardingTables,
    FT: NodeIdTable<Error = FTE>,
    FT: PathIdTable<Error = FTE>,
    FTE: std::error::Error,
{
    pub fn new(r2kad: R2Kad<C, BUCKET_SIZE>, forwarding_tables: FT) -> Self {
        let (fwtables_tx, fwtables_rx) = mpsc::channel(10);

        // forwarding tables rx
        tokio::task::spawn(async move || loop {
            let Some(req) = fwtables_rx.recv().await else {
                break;
            };
            if let Err(e) = handle_r2kad_request(&mut forwarding_tables, req) {
                log::error!(target: "forwarding_tables", "Handling an ForwardingTablesUpdate failed: {}", e);
            }
        })
    }
}
