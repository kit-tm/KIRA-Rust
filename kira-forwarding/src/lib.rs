//! KIRA fast forwarding layer implementation.
//!
//! # Architecture
//!
//!  <div>
//! <img src="../../../docs/images/forwarding.svg" />
//! </div>
//!

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub use platform::*;

pub mod domain;
pub mod platform;
pub mod tables;
pub mod underlay;

#[doc(inline)]
pub use tables::{AsyncForwardingTables, AsyncNodeIdTable, AsyncPathIdTable};
