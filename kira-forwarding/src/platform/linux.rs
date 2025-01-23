//! Linux specific platform module
use std::{ffi::OsStr, net::Ipv6Addr, process::Command};

use crate::domain::NodeId;

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

/// Encapsulates packets with a destination ip of `node_ip`
/// using the encapsulation device `kira` and sets the new outer destination to `path_id`.
///
/// This rule is used for realizing an [Encapsulate NodeIdEntry](crate::tables::NodeIdEntry::Encapsulate).
/// An existing rule is overwritten.
pub fn replace_encap_route(node_ip: &str, path_ip: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args([
            "-6", "route", "replace", node_ip, "encap", "ip6", "dst", path_ip, "dev", "kira",
        ])
        .output()
        .expect("failed to execute ip command");

    let errror_message = String::from_utf8_lossy(&output.stderr);

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip -6 route replace {} encap ip6 dst {} dev kira\" succeeded", &node_ip, &path_ip);
            Ok(())
        }
        _status => {
            log::error!(target: "linux", "command \"ip -6 route replace {} encap ip6 dst {} dev kira\" failed with status code {} and error message {}", 
            &node_ip, &path_ip, _status, errror_message);
            Err(errror_message.to_string())
        }
    }
}

/// Stops encapsulating packets with destination `node_id`.
pub fn delete_encap_route(node_ip: &str, path_ip: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args([
            "-6", "route", "del", node_ip, "encap", "ip6", "dst", path_ip, "dev", "kira",
        ])
        .output()
        .expect("failed to execute ip command");

    let errror_message = String::from_utf8_lossy(&output.stderr);

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip -6 route del {} encap ip6 dst {} dev kira\" succeeded", &node_ip, &path_ip);
            Ok(())
        }
        _status => {
            log::error!(target: "linux", "command \"ip -6 route del {} encap ip6 dst {} dev kira\" failed with status code {} and error message {}",
             &node_ip, &path_ip, _status, errror_message);
            Err(errror_message.to_string())
        }
    }
}

/// Forwards packets with destination `path_ip` as if they where destined to `next_hop_ip`.
///
/// This rule is used to determine to which underlay neighbor (`next_hop_ip`)
/// a packet with a given [PathId] is forwarded.
/// This rule is used in combination with a [forwarding_rule](add_forwarding_rule)
/// to realize A [Forward PathIdEntry](crate::tables::PathIdEntry::Forward).
/// An existing rule is overwritten.
pub fn replace_via_route(path_ip: &str, next_hop_ip: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args(["-6", "route", "replace", path_ip, "via", next_hop_ip])
        .output()
        .expect("failed to execute ip command");

    let errror_message = String::from_utf8_lossy(&output.stderr);

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip -6 route replace {} via {}\" succeeded", &path_ip, &next_hop_ip);
            Ok(())
        }
        _status => {
            log::error!(target: "linux", "command \"ip -6 route replace {} via {}\" failed with status code {} and error message {}",
             &path_ip, &next_hop_ip, _status, errror_message);
            Err(errror_message.to_string())
        }
    }
}

/// Forwards packets with destination `ip` to interface with name `interface_name` unchanged.
///
/// This is used to realize [Forward NodeIdEntry](crate::tables::NodeIdEntry::Forward).
pub fn replace_neighbor_route(ip: &str, interface_name: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args(["route", "replace", ip, "dev", interface_name])
        .output()
        .expect("failed to execute ip command");

    let errror_message = String::from_utf8_lossy(&output.stderr);

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip route replace {} dev {}\" succeeded", &ip, &interface_name);
            Ok(())
        }
        _status => {
            log::error!(target: "linux", "command \"ip route replace {} dev {}\" failed with status code {} and error message {}",
             &ip, &interface_name, _status, errror_message);
            Err(errror_message.to_string())
        }
    }
}

/// Deletes a [neighbor route](replace_neighbor_route).
pub fn delete_neighbor_route(ip: &str, interface_name: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args(["route", "del", ip, "dev", interface_name])
        .output()
        .expect("failed to execute ip command");

    let errror_message = String::from_utf8_lossy(&output.stderr);

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip route del {} dev {}\" succeeded", &ip, &interface_name);
            Ok(())
        }
        _status => {
            log::error!(target: "linux", "command \"ip route del {} dev {}\" failed with status code {} and error message {}",
             &ip, &interface_name, _status, errror_message);
            Err(errror_message.to_string())
        }
    }
}

/// Creates an interface with name `kira` for encapsulating and decapsulating
/// IPv6 packets using [GRE](https://datatracker.ietf.org/doc/rfc7676/).
///
/// This interface is needed for [encapsulation routes](replace_encap_route).
/// Decapsulation happens automatically if the outer destination IPv6 address
/// of a packets matches an address attached to the interface.
pub fn create_kira_interface() -> Result<(), String> {
    let output = Command::new("ip")
        .args(["link", "add", "name", "kira", "type", "ip6gre", "external"])
        .output()
        .expect("failed to create kira interface");

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip link add name kira type ip6gre external\" succeeded");
        }
        status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            log::error!(target: "linux", "command \"ip link add name kira type ip6gre external\" failed with status code {} and error message {}",
                        status, error_message);
            return Err(error_message.to_string());
        }
    }

    let output = Command::new("ip")
        .args(["link", "set", "addrgenmode", "none", "dev", "kira"])
        .output()
        .expect("failed to disable link local addresses on kira interface");

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip link set addrgenmode none dev kira\" succeeded");
        }
        status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            log::error!(target: "linux", "command \"ip link set addrgenmode none dev kira\" failed with status code {} and error message {}",
                        status, error_message);
            return Err(error_message.to_string());
        }
    }

    let output = Command::new("ip")
        .args(["link", "set", "kira", "up"])
        .output()
        .expect("failed to enable kira interface");

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip link set kira up\" succeeded");
        }
        status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            log::error!(target: "linux", "command \"ip link set kira up\" failed with status code {} and error message {}",
                        status, error_message);
            return Err(error_message.to_string());
        }
    }

    Ok(())
}

/// Deletes the [`kira` interface](create_kira_interface).
pub fn delete_kira_interface() -> Result<(), String> {
    let output = Command::new("ip")
        .args(["link", "delete", "kira"])
        .output()
        .expect("failed to delete kira interface");

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip link delete kira\" succeeded");
        }
        status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            log::error!(target: "linux", "command \"ip link delete kira\" failed with status code {} and error message {}",
                        status, error_message);
            return Err(error_message.to_string());
        }
    }

    Ok(())
}

/// Attaches the corresponding IPv6 address of `node_id` to the interface with name `interface`.
///
/// The `node_id` usually is the root-id of KIRA instance running on the node.
pub fn attach_node_id_ip(interface: String, node_id: &NodeId) -> Result<(), String> {
    let node_ip = Ipv6Addr::from(node_id);
    let node_ip = format!("{}", node_ip);

    let output = Command::new("ip")
        .args(["address", "add", &node_ip, "dev", &interface])
        .output()
        .expect("failed to add ip of node to interface");

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "linux", "command \"ip address add {node_ip} dev {interface}\" succeeded");
        }
        status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            // FIXME remove IPs on daemon shutdown so we don't have to ignore this
            log::warn!(target: "linux", "command \"ip address add {node_ip} dev {interface}\" failed with status code {} and error message {}",
                        status, error_message);
            //return Err(error_message.to_string());
        }
    }

    Ok(())
}
