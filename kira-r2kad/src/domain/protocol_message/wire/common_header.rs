#[cfg(feature = "binrw")]
use binrw::{
    BinRead,
    BinWrite,
};
use derive_more::Display;

use crate::domain::{
    NodeId,
    Nonce,
    ProtocolMessageKind,
    StateSeqNr,
    protocol_message::ProtocolMessageFlags,
};

/// Common Header Structure
#[derive(Debug, PartialEq, Eq, Clone, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "binrw", derive(BinRead, BinWrite), brw(big))]
#[display("v={} t={} f={:0x} dst={} src={} dom={:x} msg-id={} sseq={} deg={}",
          self.version,
          self.msg_type,
          self.msg_flags,
          self.dest_id,
          self.src_node_id,
          self.domain_id,
          self.msg_id,
          self.state_seq_num,
          self.src_node_degree,
)]
pub struct CommonHeader {
    version: u8,
    pub msg_type: ProtocolMessageKind,
    pub msg_flags: ProtocolMessageFlags,
    msg_length: u16,
    #[cfg_attr(feature = "binrw", br(map = |bytes: [u8; NodeId::SIZE]| NodeId::from(bytes)))]
    #[cfg_attr(feature = "binrw", bw(map = |id: &NodeId| id.to_be_bytes()))]
    pub dest_id: NodeId,
    #[cfg_attr(feature = "binrw", br(map = |bytes: [u8; NodeId::SIZE]| NodeId::from(bytes)))]
    #[cfg_attr(feature = "binrw", bw(map = |id: &NodeId| id.to_be_bytes()))]
    pub src_node_id: NodeId,
    pub domain_id: u64,
    pub msg_id: Nonce,
    #[cfg_attr(feature = "binrw", br(map = |ssn_raw: u32| StateSeqNr::from(ssn_raw)))]
    #[cfg_attr(feature = "binrw", bw(map = |ssn: &StateSeqNr| u32::from(*ssn)))]
    pub state_seq_num: StateSeqNr,
    pub src_node_degree: u16,
}

impl CommonHeader {
    const KIRA_PROTOCOL_VERSION: u8 = 0;

    /// create a new common header
    /// if msgid is None it is created randomly
    /// if stateseqnum is None, it is set to INVALID_SSN
    pub fn new(
        msg_type: ProtocolMessageKind,
        src: NodeId,
        dst: NodeId,
        msgid: Option<Nonce>,
        state_seq_num: impl Into<StateSeqNr>,
        src_node_degree: usize,
    ) -> Self {
        Self {
            version: Self::KIRA_PROTOCOL_VERSION,
            msg_type,
            msg_flags: ProtocolMessageFlags::default(),
            msg_length: 1 + 1 + 1 + 2 + 14 + 14 + 8 + 8 + 4 + 2, // common header length
            dest_id: dst,
            src_node_id: src,
            domain_id: 0,
            msg_id: if let Some(msg_id) = msgid {
                msg_id
            } else {
                Nonce::random()
            },
            state_seq_num: state_seq_num.into(),
            src_node_degree: if src_node_degree < u16::MAX as usize {
                src_node_degree as u16
            } else {
                u16::MAX
            },
        }
    }

    pub fn set_msg_length(&mut self, msg_len: u16) {
        self.msg_length = msg_len;
    }

    pub fn msg_type(&self) -> ProtocolMessageKind {
        self.msg_type
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn msg_flags(&self) -> ProtocolMessageFlags {
        self.msg_flags
    }

    pub fn msg_flags_mut(&mut self) -> &mut ProtocolMessageFlags {
        &mut self.msg_flags
    }

    pub fn add_to_msg_length(&mut self, msg_len: u16) {
        if self.msg_length <= u16::MAX - msg_len {
            self.msg_length += msg_len;
        } else {
            panic!(
                "maximum msg length exceeded when trying to add {} bytes to {}",
                msg_len, self.msg_length
            );
        }
    }

    pub fn msg_length(&self) -> u16 {
        self.msg_length
    }

    pub fn set_dest_id(&mut self, dst: NodeId) {
        self.dest_id = dst;
    }

    pub fn dest_id(&self) -> &NodeId {
        &self.dest_id
    }

    pub fn set_src_node_id(&mut self, src: NodeId) {
        self.src_node_id = src;
    }

    pub fn src_node_id(&self) -> &NodeId {
        &self.src_node_id
    }

    pub fn set_domain_id(&mut self, domainid: u64) {
        self.domain_id = domainid
    }

    pub fn domain_id(&self) -> u64 {
        self.domain_id
    }

    pub fn set_msg_id(&mut self, msgid: Nonce) {
        self.msg_id = msgid;
    }

    pub fn msg_id(&self) -> Nonce {
        self.msg_id
    }

    pub fn set_state_seq_num(&mut self, ssn: StateSeqNr) {
        self.state_seq_num = StateSeqNr::into(ssn);
    }

    pub fn state_seq_num(&self) -> StateSeqNr {
        self.state_seq_num
    }

    pub fn set_src_node_degree(&mut self, degree: u16) {
        self.src_node_degree = degree;
    }

    pub fn src_node_degree(&self) -> u16 {
        self.src_node_degree
    }
}
