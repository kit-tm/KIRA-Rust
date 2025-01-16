#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub use platform::*;

pub mod domain;
pub mod in_memory_tables;
pub mod native_tables;
pub mod platform;
pub mod tables;
pub mod underlay;

#[doc(inline)]
pub use tables::ForwardingTables;
