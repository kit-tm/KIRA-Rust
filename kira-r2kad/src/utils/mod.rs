//! Utility types and functions used in some parts of the application which didn't fit into any other module.

#![allow(missing_docs)]

pub use backoff_map::*;
pub use exponential_backoff::*;

pub mod backoff_map;
pub mod exponential_backoff;
pub mod rediscovery_timeout_interval;
