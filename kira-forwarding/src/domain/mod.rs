//! Domain Layer of the KIRA forwarding functionality.

pub struct PathId {
    bytes: Vec<u8>,
}

pub struct NodeId {
    bytes: [u8; SIZE],
}
