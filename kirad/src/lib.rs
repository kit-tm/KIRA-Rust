//! Library of the KIRA daemon.
//!
//! # Cargo feature flags
//!
//! - **`small_buckets`**  —  Construct routing tables with default bucket size of three.
//! - **`api`**  —  API REST service for accessing the DHT and inspecting internal data structures
//! - **`swagger_doc`**  —  Swagger documentation of the API service.
//! - **`tokio-console`**  —  Ability to enable a tracing subscriber for the [tokio-console](https://github.com/tokio-rs/console/tree/main/tokio-console)

#![forbid(unsafe_code)]
#![warn(missing_docs)]
