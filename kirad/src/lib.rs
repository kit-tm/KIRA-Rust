//! Library of the KIRA daemon.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "api")]
pub mod api;
pub mod domain;
pub mod format;
pub mod io;
pub mod r2kad;
pub mod underlay;
