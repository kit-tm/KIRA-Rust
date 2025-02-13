//! Platform specific calls.
//!
//! Currently we only support Linux but this module is build extensible.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
