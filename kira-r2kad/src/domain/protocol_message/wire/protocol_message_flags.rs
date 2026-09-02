#[cfg(feature = "binrw")]
use binrw::{
    BinRead,
    BinWrite,
};
use bitflags::bitflags;

const PROTOCOL_MSG_FLAG_EXACT: u8 = 1;
const PROTOCOL_MSG_FLAG_ENDSYSTEM: u8 = 1 << 2;
const PROTOCOL_MSG_FLAG_DIAGNOSTIC: u8 = 1 << 6;

bitflags! {
    #[derive(Debug, Default, PartialEq, Eq, Clone, Copy, Hash)]
    #[cfg_attr(
        feature = "serde",
        derive(serde::Deserialize, serde::Serialize),
    )]
    #[cfg_attr(
        feature = "binrw",
        derive(BinRead, BinWrite),
        bw(map = |flags: &Self| flags.bits()),
        br(map = |flags_raw: u8| Self::from_bits_retain(flags_raw))
    )]
    #[non_exhaustive]
    pub struct ProtocolMessageFlags: u8 {
        /// Indicates whether the dest-id is a [NodeId] and is assumed to exist.
        ///
        /// If set to 1, the [NodeId] should exist,
        /// if set to 0, the node with the closest [NodeId] will process the request.
        const Exact = PROTOCOL_MSG_FLAG_EXACT;
        /// Indicates that the originating source node is an end-system that
        /// does not perform routing or forwarding.
        ///
        /// > **Note:** The end-system mode is _not_ implemented.
        const EndSystem = PROTOCOL_MSG_FLAG_ENDSYSTEM; // TODO: Implement end-system mode
        /// Triggers explicit Error Messages instead of dropping messages silently.
        ///
        /// This flag serves mainly debugging purposes.
        const Diagnostic = PROTOCOL_MSG_FLAG_DIAGNOSTIC; // TODO: Support Diagnostic flag

        // The source may set any bits
        const _ = !0;
    }
}
