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
//!
//! # Cargo feature flags
//!
//! - **`small_buckets`**  —  Construct routing tables with default bucket size of three.
//! - **`udp-tokio`**  —  Async implementations for sending and receiving protocol messages using tokio sockets
//! - **`format-mp`**  —  Protocol Message Format support: Message Pack
//! - **`format-json`**  —  Protocol Message Format support: Json
//! - **`api`**  —  API REST service for accessing the DHT and inspecting internal data structures
//! - **`swagger_doc`**  —  Swagger documentation of the API service.
//! - **`tokio-console`**  —  Ability to enable a tracing subscriber for the [tokio-console](https://github.com/tokio-rs/console/tree/main/tokio-console)

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "api")]
pub mod api;
pub mod domain;
pub mod format;
pub mod io;
pub mod kira;
pub mod underlay;

pub use kira::Kira;
