//! Platform specific calls.
//!
//! Currently we only support Linux but this module is build extensible.

#[cfg(target_os = "linux")]
pub mod linux;
// `linux` is empty without these features, which would make the glob re-export unused.
#[cfg(all(target_os = "linux", any(feature = "nft", feature = "srv6")))]
pub use linux::*;
