//! IPv6 prefixes of the addresses derived from [NodeId]s and [PathId]s.

use std::{
    fmt::{
        Display,
        Formatter,
    },
    net::Ipv6Addr,
    str::FromStr,
};

use derive_more::{
    Display,
    Error,
};

use crate::domain::{
    NodeId,
    NodeIdSubnet,
    PathId,
};

/// An IPv6 prefix of length 16.
///
/// The remaining 112 bits of an address hold a [NodeId] or the first 112 bits of a [PathId].
///
/// Parses from and displays as `<address>/16`, e.g. `fc00::/16`.
/// When parsing, the prefix length may be omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv6Prefix16(u16);

impl Ipv6Prefix16 {
    /// Length of the prefix in bits.
    pub const LENGTH: u8 = 16;

    /// Creates the prefix from its 16 bits, e.g. `0xfc00` for `fc00::/16`.
    pub const fn new(prefix: u16) -> Self {
        Self(prefix)
    }

    /// Returns the 16 bits of the prefix.
    pub const fn bits(&self) -> u16 {
        self.0
    }

    /// Returns the address with this prefix and the given 112 bits.
    const fn address(&self, suffix: u128) -> Ipv6Addr {
        const SUFFIX_BITS: u32 = Ipv6Addr::BITS - Ipv6Prefix16::LENGTH as u32;
        const SUFFIX_MASK: u128 = (1 << SUFFIX_BITS) - 1;
        Ipv6Addr::from_bits(((self.0 as u128) << SUFFIX_BITS) | (suffix & SUFFIX_MASK))
    }
}

impl Display for Ipv6Prefix16 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.address(0), Self::LENGTH)
    }
}

/// Error when parsing an [Ipv6Prefix16].
#[derive(Debug, Display, Error, PartialEq, Eq)]
pub enum Ipv6Prefix16Error {
    /// The address part is no valid IPv6 address.
    #[display("invalid IPv6 address: {_0}")]
    InvalidAddress(#[error(not(source))] String),
    /// The prefix length is not 16.
    #[display("prefix length must be 16, got {_0}")]
    InvalidLength(#[error(not(source))] String),
    /// Bits after the first 16 bits are set.
    #[display("only the first 16 bits may be set: {_0}")]
    HostBitsSet(#[error(not(source))] Ipv6Addr),
}

impl FromStr for Ipv6Prefix16 {
    type Err = Ipv6Prefix16Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (address, length) = s.split_once('/').unwrap_or((s, "16"));
        if length.parse::<u8>().ok() != Some(Self::LENGTH) {
            return Err(Ipv6Prefix16Error::InvalidLength(length.to_string()));
        }
        let address = Ipv6Addr::from_str(address)
            .map_err(|_| Ipv6Prefix16Error::InvalidAddress(address.to_string()))?;

        let [first, rest @ ..] = address.segments();
        if rest.iter().any(|segment| *segment != 0) {
            return Err(Ipv6Prefix16Error::HostBitsSet(address));
        }
        Ok(Self(first))
    }
}

/// Default prefix of NodeIPs: `fc00::/16`.
pub const DEFAULT_NODE_IP_PREFIX: Ipv6Prefix16 = Ipv6Prefix16::new(0xfc00);
/// Default prefix of PathIPs: `fcaa::/16`.
pub const DEFAULT_PATH_IP_PREFIX: Ipv6Prefix16 = Ipv6Prefix16::new(0xfcaa);

/// Prefixes of the IPv6 addresses derived from [NodeId]s (NodeIPs) and [PathId]s (PathIPs).
///
/// All nodes of a KIRA network must use the same prefixes,
/// as NodeIPs and PathIPs are used as destination addresses of packets between nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpPrefixes {
    node_ip: Ipv6Prefix16,
    path_ip: Ipv6Prefix16,
}

/// Error when creating [IpPrefixes].
#[derive(Debug, Display, Error, PartialEq, Eq)]
#[display("NodeIP and PathIP prefix must differ, both are {_0}")]
pub struct EqualPrefixesError(#[error(not(source))] Ipv6Prefix16);

impl IpPrefixes {
    /// Creates the prefixes, which must differ to tell NodeIPs and PathIPs apart.
    pub fn new(node_ip: Ipv6Prefix16, path_ip: Ipv6Prefix16) -> Result<Self, EqualPrefixesError> {
        if node_ip == path_ip {
            return Err(EqualPrefixesError(node_ip));
        }
        Ok(Self { node_ip, path_ip })
    }

    /// Returns the prefix of NodeIPs.
    pub const fn node_ip_prefix(&self) -> Ipv6Prefix16 {
        self.node_ip
    }

    /// Returns the prefix of PathIPs.
    pub const fn path_ip_prefix(&self) -> Ipv6Prefix16 {
        self.path_ip
    }

    /// Returns the NodeIP of the [NodeId].
    pub fn node_ip(&self, node_id: &NodeId) -> Ipv6Addr {
        self.node_ip.address(u128::from(*node_id))
    }

    /// Returns the subnet of NodeIPs equivalent to the [NodeIdSubnet].
    ///
    /// The second value is the [prefix length](NodeIdSubnet::ipv6_subnet_prefix_length).
    pub fn node_ip_subnet(&self, subnet: &NodeIdSubnet) -> (Ipv6Addr, u8) {
        (
            self.node_ip(subnet.node_id()),
            subnet.ipv6_subnet_prefix_length(),
        )
    }

    /// Returns the PathIP of the [PathId], which contains the first 112 bits of the [PathId].
    pub fn path_ip(&self, path_id: &PathId) -> Ipv6Addr {
        let mut bytes = [0; 16];
        bytes[2..].copy_from_slice(&path_id.as_ref()[..14]);
        self.path_ip.address(u128::from_be_bytes(bytes))
    }
}

impl Default for IpPrefixes {
    fn default() -> Self {
        Self {
            node_ip: DEFAULT_NODE_IP_PREFIX,
            path_ip: DEFAULT_PATH_IP_PREFIX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefix_with_and_without_length() {
        assert_eq!("fc00::/16".parse(), Ok(Ipv6Prefix16::new(0xfc00)));
        assert_eq!("fd12::".parse(), Ok(Ipv6Prefix16::new(0xfd12)));
    }

    #[test]
    fn rejects_invalid_prefixes() {
        assert!(matches!(
            "fc00::/48".parse::<Ipv6Prefix16>(),
            Err(Ipv6Prefix16Error::InvalidLength(_))
        ));
        assert!(matches!(
            "fc00::1/16".parse::<Ipv6Prefix16>(),
            Err(Ipv6Prefix16Error::HostBitsSet(_))
        ));
        assert!(matches!(
            "fc00:1::/16".parse::<Ipv6Prefix16>(),
            Err(Ipv6Prefix16Error::HostBitsSet(_))
        ));
        assert!(matches!(
            "fcxx::/16".parse::<Ipv6Prefix16>(),
            Err(Ipv6Prefix16Error::InvalidAddress(_))
        ));
    }

    #[test]
    fn display_round_trips() {
        assert_eq!(DEFAULT_NODE_IP_PREFIX.to_string(), "fc00::/16");
        assert_eq!(DEFAULT_PATH_IP_PREFIX.to_string(), "fcaa::/16");
        assert_eq!(
            DEFAULT_PATH_IP_PREFIX.to_string().parse(),
            Ok(DEFAULT_PATH_IP_PREFIX)
        );
    }

    #[test]
    fn prefixes_must_differ() {
        assert_eq!(
            IpPrefixes::new(DEFAULT_NODE_IP_PREFIX, DEFAULT_NODE_IP_PREFIX),
            Err(EqualPrefixesError(DEFAULT_NODE_IP_PREFIX))
        );
    }

    #[test]
    fn node_ip_uses_configured_prefix() {
        let node_id: NodeId = "0123456789abcdef0123456789ab".parse().unwrap();
        let prefixes = IpPrefixes::new(Ipv6Prefix16::new(0xfd00), DEFAULT_PATH_IP_PREFIX).unwrap();

        assert_eq!(
            IpPrefixes::default().node_ip(&node_id),
            "fc00:0123:4567:89ab:cdef:0123:4567:89ab"
                .parse::<Ipv6Addr>()
                .unwrap()
        );
        assert_eq!(
            prefixes.node_ip(&node_id),
            "fd00:0123:4567:89ab:cdef:0123:4567:89ab"
                .parse::<Ipv6Addr>()
                .unwrap()
        );
    }

    #[test]
    fn node_ip_subnet_keeps_prefix_length() {
        let node_id: NodeId = "0123456789abcdef0123456789ab".parse().unwrap();
        let prefixes = IpPrefixes::new(Ipv6Prefix16::new(0xfd00), DEFAULT_PATH_IP_PREFIX).unwrap();

        assert_eq!(
            prefixes.node_ip_subnet(&NodeIdSubnet::new(node_id)),
            (prefixes.node_ip(&node_id), 128)
        );
    }

    #[test]
    fn path_ip_uses_configured_prefix() {
        let path_id: PathId = "00112233445566778899aabbccddeeff".parse().unwrap();
        let prefixes = IpPrefixes::new(DEFAULT_NODE_IP_PREFIX, Ipv6Prefix16::new(0xfdaa)).unwrap();

        assert_eq!(
            IpPrefixes::default().path_ip(&path_id),
            "fcaa:0011:2233:4455:6677:8899:aabb:ccdd"
                .parse::<Ipv6Addr>()
                .unwrap()
        );
        assert_eq!(
            prefixes.path_ip(&path_id),
            "fdaa:0011:2233:4455:6677:8899:aabb:ccdd"
                .parse::<Ipv6Addr>()
                .unwrap()
        );
    }
}
