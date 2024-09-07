use futures::TryStreamExt;
use rtnetlink::new_connection;
use std::process::Command;

pub async fn create_kira_interface_async() -> Result<(), rtnetlink::Error> {
    let (connection, handle, _) = new_connection().unwrap();
    tokio::spawn(connection);

    handle
        .link()
        .add()
        .dummy("kira".to_string())
        .execute()
        .await?;

    let kira = handle
        .link()
        .get()
        .match_name("kira".to_string())
        .execute()
        .try_next()
        .await?
        .expect("kira interface should still exist");

    handle.link().set(kira.header.index).up().execute().await
}

pub fn create_nid_default_route() -> Result<(), String> {
    let output = Command::new("ip")
        .args(["route", "add", "fc00::/16", "dev", "lo"])
        .output()
        .expect("Failed to create default route");

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "ebpf", "command \"ip route add fc00::/16 dev lo\" succeeded");
        }
        status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            log::error!(target: "ebpf", "command \"ip route add fc00::/16 dev lo\" failed with status code {} and error message {}",
                        status, error_message);
            return Err(error_message.to_string());
        }
    }

    Ok(())
}

pub fn delete_nid_default_route() -> Result<(), String> {
    let output = Command::new("ip")
        .args(["route", "delete", "fc00::/16"])
        .output()
        .expect("Failed to delete default route");

    match output
        .status
        .code()
        .expect("ip command externally terminated")
    {
        0 => {
            log::trace!(target: "ebpf", "command \"ip route delete fc00::/16\" succeeded");
        }
        status => {
            let error_message = String::from_utf8_lossy(&output.stderr);
            log::error!(target: "ebpf", "command \"ip route delete fc00::/16\" failed with status code {} and error message {}",
                        status, error_message);
            return Err(error_message.to_string());
        }
    }

    Ok(())
}
