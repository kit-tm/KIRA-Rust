//! Library of the KIRA daemon.
//!
//! # Logging Targets
//!
//! Logging is implemented through the [log](https://crates.io/crates/log) crate.
//! While sometimes default logging targets based on module structure are used, some special
//! logging targets have been added.
//!
//! - `r2kad`: Information about the event processing by the R²/KAD instance.
//! - `message_sender`: Information about sending protocol messages.
//! - `message_receiver`: Information about receiving protocol messages.
//! - `underlay_observer`: Updates to the nodes underlay neighborhood.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "api")]
pub mod api;
pub mod domain;
pub mod format;
pub mod io;
pub mod r2kad;
pub mod underlay;
