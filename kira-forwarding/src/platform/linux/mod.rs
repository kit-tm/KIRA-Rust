//! Linux specific platform module
use std::{ffi::OsStr, net::Ipv6Addr, process::Command};

#[cfg(feature = "nft")]
pub mod netlink;

/// Loads a config from a path for [nftables](https://netfilter.org/projects/nftables/)
/// and tries to apply it.
///
/// If the command fails the captured error message is returned as [Err].
pub fn load_nft_config<S: AsRef<OsStr>>(path: S) -> Result<(), String> {
    let output = Command::new("nft").arg("-f").arg(path).output().unwrap();
    match output.status.code().expect("failed to execute nft command") {
        0 => {
            log::debug!(target: "linux", "Loaded nftables successfully");
            Ok(())
        }
        _status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            log::error!(target: "linux", "Loaded nftables config with status code {:?} and error message {:?}", _status, error_message);
            Err(error_message.to_string())
        }
    }
}

/// Adds a destination based forwarding rule.
///
/// This rule forwards IPv6 packets destined to `from` using the new destination.
pub fn add_forwarding_rule(from: Ipv6Addr, to: Ipv6Addr) -> Result<(), String> {
    let from_addr = from.to_string();
    let to_addr = to.to_string();
    let output = Command::new("nft")
        .args([
            "add",
            "element",
            "ip6",
            "kira",
            "forwardmap",
            &format!("{{\"{}\" : \"{}\"}}", from_addr, to_addr),
        ])
        .output()
        .expect("failed to execute nft command");

    let errror_message = String::from_utf8_lossy(&output.stderr);

    match output
        .status
        .code()
        .expect("nft command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"nft add element ip6 kira forwardmap {}\" succeeded",
            format!("{{\"{}\" : \"{}\"}}", from_addr, to_addr));
            Ok(())
        }
        _status => {
            log::error!(target: "linux", "command \"nft add element ip6 kira forwardmap {}\" failed with status code {} and error message {}",
            format!("{{\"{}\" : \"{}\"}}", from_addr, to_addr), _status, errror_message);
            Err(errror_message.to_string())
        }
    }
}

/// Updates an existing forwarding rule by first deleting it.
///
/// See [add_forwarding_rule] for more details.
pub fn update_forwarding_rule(from: Ipv6Addr, to: Ipv6Addr) -> Result<(), String> {
    // TODO make this (and the other nft operations) atomic with: printf <config> | nft -f -
    delete_forwarding_rule(from)?;
    add_forwarding_rule(from, to)
}

/// Deletes an existing forwarding rule.
pub fn delete_forwarding_rule(from: Ipv6Addr) -> Result<(), String> {
    let from_addr = from.to_string();
    let output = Command::new("nft")
        .args([
            "delete",
            "element",
            "ip6",
            "kira",
            "forwardmap",
            &format!("{{\"{}\"}}", from_addr),
        ])
        .output()
        .expect("failed to execute nft command");

    let errror_message = String::from_utf8_lossy(&output.stderr);

    match output
        .status
        .code()
        .expect("nft command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"nft delete element ip6 kira forwardmap {}\" succeeded", format!("{{\"{}\"}}", from_addr));
            Ok(())
        }
        _status => {
            log::error!(target: "linux", "command \"nft delete element ip6 kira forwardmap {}\" failed with status code {} and error message {}", 
            format!("{{\"{}\"}}", from_addr), _status, errror_message);
            Err(errror_message.to_string())
        }
    }
}
