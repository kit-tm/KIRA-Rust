use std::{ffi::OsStr, net::Ipv6Addr, process::Command};

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

pub fn update_forwarding_rule(from: Ipv6Addr, to: Ipv6Addr) -> Result<(), String> {
    // TODO make this (and the other nft operations) atomic with: printf <config> | nft -f -
    delete_forwarding_rule(from)?;
    add_forwarding_rule(from, to)
}

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

pub fn replace_encap_route(node_ip: &str, path_ip: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args([
            "-6", "route", "replace", &node_ip, "encap", "ip6", "dst", &path_ip, "dev", "kira",
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

pub fn delete_encap_route(node_ip: &str, path_ip: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args([
            "-6", "route", "del", &node_ip, "encap", "ip6", "dst", &path_ip, "dev", "kira",
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

pub fn replace_via_route(path_ip: &str, next_hop_ip: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args(["-6", "route", "replace", &path_ip, "via", &next_hop_ip])
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

pub fn replace_neighbor_route(ip: &str, interface_name: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args(["route", "replace", &ip, "dev", &interface_name])
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

pub fn delete_neighbor_route(ip: &str, interface_name: &str) -> Result<(), String> {
    let output = Command::new("ip")
        .args(["route", "del", &ip, "dev", &interface_name])
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
