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

#[cfg(all(test, feature = "binrw"))]
mod tests {
    use std::io::Cursor;

    use binrw::{
        BinRead,
        BinWrite,
    };

    use super::*;

    #[test]
    fn flag_bits_have_wire_values() {
        assert_eq!(ProtocolMessageFlags::Exact.bits(), 0b0000_0001);
        assert_eq!(ProtocolMessageFlags::EndSystem.bits(), 0b0000_0100);
        assert_eq!(ProtocolMessageFlags::Diagnostic.bits(), 0b0100_0000);
    }

    #[test]
    fn binrw_round_trip_preserves_flags() {
        let flags = ProtocolMessageFlags::Exact | ProtocolMessageFlags::Diagnostic;

        let mut buffer = Cursor::new(Vec::new());
        flags
            .write_be(&mut buffer)
            .expect("writing flags must work");
        assert_eq!(
            buffer.into_inner(),
            vec![0b0100_0001],
            "flags are written as a single byte"
        );

        let mut buffer = Cursor::new(vec![0b0100_0001u8]);
        let read_back =
            ProtocolMessageFlags::read_be(&mut buffer).expect("reading flags must work");
        assert_eq!(read_back, flags);
    }

    #[test]
    fn binrw_round_trip_retains_unknown_bits() {
        // The source may set any bits; unknown bits must survive the wire
        let raw: u8 = 0b1011_1111;

        let mut buffer = Cursor::new(vec![raw]);
        let flags = ProtocolMessageFlags::read_be(&mut buffer).expect("reading flags must work");

        let mut buffer = Cursor::new(Vec::new());
        flags
            .write_be(&mut buffer)
            .expect("writing flags must work");
        assert_eq!(
            buffer.into_inner(),
            vec![raw],
            "unknown bits must not be dropped"
        );
    }
}
