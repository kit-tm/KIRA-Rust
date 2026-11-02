//! Integration library of the KIRA daemon.
//!
//! See [Kira] for the main component that orchestrates all other components
//! necessary to run a KIRA daemon.
//!
//! # Architecture
//!
//!  <div>
//! <img src="../../../docs/images/lib.svg" />
//! </div>
//!
//! # Logging Targets
//!
//! Logging is implemented through the [log](https://crates.io/crates/log) crate.
//! While sometimes default logging targets based on module structure are used, some special
//! logging targets have been added.
//!
//! - `message_sender`: Information about sending protocol messages.
//! - `message_receiver`: Information about receiving protocol messages.
//! - `underlay_observer`: Updates to the nodes underlay neighborhood.
//!
//! # Cargo feature flags
//!
//! - **`udp-tokio`**  —  Async implementations for sending and receiving protocol messages using tokio sockets
//! - **`format-binrw`**  —  Protocol Message Format support: BinRW
//! - **`format-cbor`**  —  Protocol Message Format support: CBOR (Concise Binary Object Notation)
//! - **`format-mp`**  —  Protocol Message Format support: Message Pack
//! - **`format-json`**  —  Protocol Message Format support: Json
//! - **`api`**  —  API REST service for accessing the DHT and inspecting internal data structures
//! - **`swagger_doc`**  —  Swagger documentation of the API service.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "api")]
pub mod api;
pub mod domain;
pub mod format;
pub mod io;
pub mod kira;
pub mod underlay;

#[doc(inline)]
pub use kira::Kira;
