use std::time::Instant;

use kira_lib::domain::protocol_event::{Input, Output};
use kira_lib::R2Kad;

#[tokio::main]
async fn main() {
    let mut r2kad = R2Kad::new();

    loop {
        let timeout = match r2kad.poll_output().unwrap() {
            Output::Timeout(v) => v,
            Output::SendProtocolMessage(message, destination) => {
                // TODO: Send data to remote peer.
                continue; // poll again
            }
            Output::UpdateForwardingTables(update_req) => {
                // TODO: Update the forwarding tables.
                continue; // poll again
            }
        };

        // Wait for two types of events:
        //   1. Network input or Debug requests
        //   2. Timeout
        match tokio::time::timeout(Instant::now().duration_since(timeout), async move {
            // TODO: Receive data from remote peers.
            todo!("receive protocol messages")
        })
        .await
        {
            Ok(input) => r2kad.receive_event(input).unwrap(),
            Err(_) => continue, // poll again
        }
    }
}
