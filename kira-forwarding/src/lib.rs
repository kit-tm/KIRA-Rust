//! KIRA fast forwarding layer implementation.
//!
//! The [forwarding tables](tables) are filled by the routing tier with entries based on
//! NodeIds and PathIds. There are two implementations for Linux that differ in the
//! encapsulation carrying the PathIds:
//!
//! - [`NativeFwdTables`](tables::native_tables::NativeFwdTables) (feature `nft`):
//!   GRE encapsulation, PathIds are handled by nftables. It carries a single PathId, so the
//!   routing tier has to use the one-segment scheme
//!   ([`PathSegmentation::OneSegment`](kira_r2kad::domain::PathSegmentation::OneSegment)).
//! - [`Srv6FwdTables`](tables::srv6_tables::Srv6FwdTables) (feature `srv6`):
//!   SRv6 encapsulation, PathIds are handled by an eBPF program. Paths to contacts are split
//!   into up to two path segments, each identified by its own PathId.
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
pub use tables::{
    AsyncForwardingTables,
    AsyncNodeIdTable,
    AsyncPathIdTable,
};
