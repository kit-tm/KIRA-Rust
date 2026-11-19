//! Library of the KIRA daemon.
//!
//! # Cargo feature flags
//!
//! - **`small_buckets`**  —  Construct routing tables with default bucket size of three.
//! - **`api`**  —  API REST service for accessing the DHT and inspecting internal data structures
//! - **`swagger_doc`**  —  Swagger documentation of the API service.
//! - **`tokio-console`**  —  Ability to enable a tracing subscriber for the [tokio-console](https://github.com/tokio-rs/console/tree/main/tokio-console)
//! - **`format-binrw`**  —  Protocol Message Format support: BinRW
//! - **`format-cbor`**  —  Protocol Message Format support: CBOR (Concise Binary Object Notation)
//! - **`format-mp`**  —  Protocol Message Format support: Message Pack
//! - **`format-json`**  —  Protocol Message Format support: Json
//! - **`srv6`**  —  SRv6-based fast forwarding using an eBPF program
//!   (building requires a nightly toolchain with `rust-src` and `bpf-linker`)

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use clap::ValueEnum;
use kira_lib::format::ProtocolMessageFormat;

#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Determines the encapsulation used by the fast forwarding of data packets.
///
/// Must be the same for all nodes, as nodes only handle the encapsulation they use themselves
/// and as the modes differ in how paths are split into path segments.
pub enum ForwardingMode {
    /// [GRE](https://datatracker.ietf.org/doc/rfc7676/) encapsulation with a single PathID
    /// for the whole path (one-segment scheme), PathIDs are handled by nftables.
    #[default]
    Gre,
    #[cfg(feature = "srv6")]
    /// [SRv6](https://datatracker.ietf.org/doc/rfc8754/) encapsulation with up to two path
    /// segments (two-segment scheme): an outer IPv6 header and, for two path segments,
    /// a Reduced Segment Routing Header.
    /// PathIDs are handled by an eBPF program.
    Srv6,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
/// Determines the serialization format of the protocol messages of R²/KAD.
///
/// Should be the same for all nodes, as nodes don't support deserializing
/// multiple formats at once.
pub enum R2KadMessageFormat {
    #[cfg(feature = "format-binrw")]
    /// Binary serialization using the `binrw` format.
    Binrw,
    #[cfg(feature = "format-json")]
    /// [JavaScript object notation](https://www.json.org) message format.
    Json,
    #[cfg(feature = "format-cbor")]
    /// [Concise Binary Object Representation (CBOR)](https://datatracker.ietf.org/doc/html/rfc8949) message format.
    ///
    /// CBOR is very efficient and a platform independent encoding, esp. used in IoT contexts
    CBOR,
    #[cfg(feature = "format-mp")]
    /// [MessagePack](https://msgpack.org/) message format.
    ///
    /// MessagePack is similar to JSON but more compact and
    /// should be preferred unless readability is a concern.
    MP,
}

impl Default for R2KadMessageFormat {
    fn default() -> Self {
        #[cfg(feature = "format-binrw")]
        return Self::Binrw;
        #[cfg(all(not(feature = "format-binrw"), feature = "format-cbor"))]
        return Self::CBOR;
        #[cfg(all(
            not(feature = "format-binrw"),
            not(feature = "format-cbor"),
            feature = "format-mp"
        ))]
        return Self::MP;
        #[cfg(all(
            not(feature = "format-binrw"),
            not(feature = "format-cbor"),
            not(feature = "format-mp"),
            feature = "format-json"
        ))]
        return Self::JSON;
        #[cfg(all(
            not(feature = "format-binrw"),
            not(feature = "format-cbor"),
            not(feature = "format-mp"),
            not(feature = "format-json")
        ))]
        compile_error!(
            "At least one serialization format feature must be enabled: [\"format-binrw\", \"format-cbor\", \"format-mp\", \"format-json\"]."
        );
    }
}

impl From<R2KadMessageFormat> for ProtocolMessageFormat {
    fn from(r2kad_fmt: R2KadMessageFormat) -> Self {
        match r2kad_fmt {
            #[cfg(feature = "format-binrw")]
            R2KadMessageFormat::Binrw => ProtocolMessageFormat::Binrw,
            #[cfg(feature = "format-json")]
            R2KadMessageFormat::Json => ProtocolMessageFormat::Json,
            #[cfg(feature = "format-cbor")]
            R2KadMessageFormat::CBOR => ProtocolMessageFormat::CBOR,
            #[cfg(feature = "format-mp")]
            R2KadMessageFormat::MP => ProtocolMessageFormat::MessagePack,
        }
    }
}
