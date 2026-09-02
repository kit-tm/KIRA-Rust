//! Definitions of the wire-format of R²/KAD [ProtocolMessages].
//!
//! Contrary to the internal representations of [ProtocolMessages],
//! the wire-format is defined in the I-D and expected to be more stable.
//!
//!
//! [ProtocolMessages]: crate::domain::ProtocolMessage

use crate::domain::{
    NodeId,
    Nonce,
    StateSeqNr,
};

mod common_header;
mod protocol_message_flags;
mod protocol_message_kind;
mod protocol_object;

pub use self::{
    common_header::CommonHeader,
    protocol_message_flags::ProtocolMessageFlags,
    protocol_message_kind::ProtocolMessageKind,
    protocol_object::{
        CommonObjectHeader,
        ProtocolObjectType,
        RTableRequestTypeValue,
    },
};

pub trait WireFormatMessage {
    fn common_header(&self) -> &CommonHeader;
    fn common_header_mut(&mut self) -> &mut CommonHeader;

    fn msg_flags(&self) -> ProtocolMessageFlags {
        self.common_header().msg_flags
    }

    fn msg_flags_mut(&mut self) -> &mut ProtocolMessageFlags {
        &mut self.common_header_mut().msg_flags
    }

    fn set_dest_id(&mut self, dst: NodeId) {
        self.common_header_mut().set_dest_id(dst);
    }

    fn dest_id(&self) -> &NodeId {
        self.common_header().dest_id()
    }

    fn set_src_node_id(&mut self, src: NodeId) {
        self.common_header_mut().set_src_node_id(src);
    }

    fn src_node_id(&self) -> &NodeId {
        self.common_header().src_node_id()
    }

    fn set_domain_id(&mut self, domain_id: u64) {
        self.common_header_mut().set_domain_id(domain_id);
    }

    fn domain_id(&self) -> u64 {
        self.common_header().domain_id()
    }

    fn set_msg_id(&mut self, msgid: Nonce) {
        self.common_header_mut().set_msg_id(msgid);
    }

    fn msg_id(&self) -> Nonce {
        self.common_header().msg_id()
    }

    fn set_state_seq_num(&mut self, ssn: StateSeqNr) {
        self.common_header_mut().state_seq_num = ssn;
    }

    fn state_seq_num(&self) -> StateSeqNr {
        self.common_header().state_seq_num()
    }

    fn set_src_node_degree(&mut self, degree: u16) {
        self.common_header_mut().src_node_degree = degree;
    }

    fn src_node_degree(&self) -> u16 {
        self.common_header().src_node_degree()
    }
}
