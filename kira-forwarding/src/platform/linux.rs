//! Linux specific platform module

#[cfg(feature = "nft")]
pub mod netlink;

#[cfg(feature = "srv6")]
pub mod ebpf;
