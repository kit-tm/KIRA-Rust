use std::fmt::{Display, Formatter};
use std::io::Write;
use std::time::Duration;

use r2kad_lib::messaging::ProtocolMessage;

#[derive(Debug, Eq, PartialEq, Hash)]
pub enum MessageType {
    Hello,
    PNDiscReq,
    PNDiscRsp,
    QueryRouteReq,
    QueryRouteRsp,
    FindNodeReq,
    FindNodeRsp,
    Error,
    PathSetupReq,
    PathTeardownReq,
    UpdateRouteReq,
    ProbeReq,
    ProbeRsp,
    StoreReq,
    StoreRsp,
    FetchReq,
    FetchRsp
}

impl<'a> From<&'a ProtocolMessage> for MessageType {
    fn from(message: &'a ProtocolMessage) -> Self {
        match message {
            ProtocolMessage::Hello(_) => MessageType::Hello,
            ProtocolMessage::PNDiscReq(_) => MessageType::PNDiscReq,
            ProtocolMessage::PNDiscRsp(_) => MessageType::PNDiscRsp,
            ProtocolMessage::QueryRouteReq(_) => MessageType::QueryRouteReq,
            ProtocolMessage::QueryRouteRsp(_) => MessageType::QueryRouteRsp,
            ProtocolMessage::FindNodeReq(_) => MessageType::FindNodeReq,
            ProtocolMessage::FindNodeRsp(_) => MessageType::FindNodeRsp,
            ProtocolMessage::ProbeReq(_) => MessageType::ProbeReq,
            ProtocolMessage::ProbeRsp(_) => MessageType::ProbeRsp,
            ProtocolMessage::PathSetupReq(_) => MessageType::PathSetupReq,
            ProtocolMessage::PathTeardownReq(_) => MessageType::PathTeardownReq,
            ProtocolMessage::UpdateRouteReq(_) => MessageType::UpdateRouteReq,
            ProtocolMessage::Error(_) => MessageType::Error,
            ProtocolMessage::StoreReq(_) => MessageType::StoreReq,
            ProtocolMessage::StoreRsp(_) => MessageType::StoreRsp,
            ProtocolMessage::FetchReq(_) => MessageType::FetchReq,
            ProtocolMessage::FetchRsp(_) => MessageType::FetchRsp
        }
    }
}

impl Display for MessageType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hello => write!(f, "PNHello"),
            Self::PNDiscReq => write!(f, "PNDiscReq"),
            Self::PNDiscRsp => write!(f, "PNDiscRsp"),
            Self::QueryRouteReq => write!(f, "QueryRouteReq"),
            Self::QueryRouteRsp => write!(f, "QueryRouteRsp"),
            Self::FindNodeReq => write!(f, "FindNodeReq"),
            Self::FindNodeRsp => write!(f, "FindNodeRsp"),
            Self::Error => write!(f, "Error"),
            Self::PathSetupReq => write!(f, "PathSetupReq"),
            Self::PathTeardownReq => write!(f, "PathTeardownReq"),
            Self::UpdateRouteReq => write!(f, "UpdateRouteReq"),
            Self::ProbeReq => write!(f, "ProbeReq"),
            Self::ProbeRsp => write!(f, "ProbeRsp"),
            MessageType::StoreReq => write!(f, "StoreReq"),
            MessageType::StoreRsp => write!(f, "StoreRsp"),
            MessageType::FetchReq => write!(f, "FetchReq"),
            MessageType::FetchRsp => write!(f, "FetchRsp"),
        }
    }
}

#[derive(Debug)]
pub struct BenchmarkEntry {
    message_type: MessageType,
    took_time: Duration,
}

impl BenchmarkEntry {
    pub fn new(message: &ProtocolMessage, time: Duration) -> Self {
        Self {
            message_type: MessageType::from(message),
            took_time: time,
        }
    }
}

pub struct BenchmarkLog {
    entries: Vec<BenchmarkEntry>,
}

impl BenchmarkLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, entry: BenchmarkEntry) {
        self.entries.push(entry);
    }

    /// Drains the log by appending its contents to the given file.
    ///
    /// Outputs in CSV Format.
    pub fn append_to<W: Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        for entry in self.entries.drain(..) {
            writeln!(
                writer,
                "{},{}",
                entry.message_type,
                entry.took_time.as_nanos()
            )?;
        }

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn append_bench<W: Write>(&mut self, entry: BenchmarkEntry, writer: &mut W) {
        log::trace!("Entry added {:?}", entry);
        self.add(entry);
        if self.len() >= 1000 || self.len() == usize::MAX {
            self.flush(writer);
        }
    }

    pub fn flush<W: Write>(&mut self, writer: &mut W) {
        if let Err(e) = self.append_to(writer) {
            log::error!("Failed to append log: {}", e);
        }
        if let Err(e) = writer.flush() {
            log::error!("Failed to flush writer: {}", e);
        }
    }
}
