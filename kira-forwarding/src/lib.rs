pub use platform::*;

pub mod domain;
pub mod in_memory_tables;
pub mod native_tables;
pub mod platform;
pub mod tables;

#[doc(inline)]
pub use tables::ForwardingTables;
