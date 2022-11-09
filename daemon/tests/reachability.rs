use std::collections::HashMap;
use std::num::NonZeroU64;
use std::time::Duration;

use petgraph::dot::{Config, Dot};
use petgraph::prelude::*;

use r2kad_lib::domain::NodeId;
use r2kad_lib::messaging::{FindNodeReqData, ProtocolMessage};

use crate::common::Network;

mod common;

#[test]
#[ntest::timeout(300000)]
fn reachability() {
    common::setup("reachability");

    // Build graph
    let mut graph = Graph::<NodeId, (), Undirected, u32>::new_undirected();

    let node_ids = [
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
        NodeId::random(),
    ];

    for node_id in &node_ids {
        graph.add_node(node_id.clone());
    }

    graph.add_edge(NodeIndex::new(0), NodeIndex::new(1), ());
    graph.add_edge(NodeIndex::new(1), NodeIndex::new(2), ());
    graph.add_edge(NodeIndex::new(2), NodeIndex::new(3), ());
    graph.add_edge(NodeIndex::new(3), NodeIndex::new(4), ());
    graph.add_edge(NodeIndex::new(4), NodeIndex::new(5), ());
    graph.add_edge(NodeIndex::new(5), NodeIndex::new(6), ());
    graph.add_edge(NodeIndex::new(6), NodeIndex::new(7), ());
    graph.add_edge(NodeIndex::new(7), NodeIndex::new(8), ());
    graph.add_edge(NodeIndex::new(8), NodeIndex::new(9), ());

    log::info!(
        "Using network: {:?}",
        Dot::with_config(&graph, &[Config::EdgeNoLabel])
    );

    let network = Network::from(graph);

    let mut network = network.start();

    std::thread::sleep(Duration::from_secs(30));

    let mut failed_requests: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut successful_requests: HashMap<NodeId, Vec<NodeId>> = HashMap::new();

    for source in node_ids.clone() {
        for target in node_ids.clone() {
            if source == target {
                continue;
            }

            let node = network
                .node_mut(&source)
                .expect("no node with added id found");

            let result = node.send_find_and_wait_for_response(
                FindNodeReqData {
                    exact: true,
                    neighborhood: NonZeroU64::new(20).unwrap(),
                    target: target.clone(),
                },
                Duration::from_secs(1),
            );

            match result {
                Ok((message, _)) => {
                    assert!(
                        matches!(message, ProtocolMessage::FindNodeRsp(_)),
                        "Received response which is not a FindNodeRsp: {:?}",
                        message
                    );

                    log::info!("Node {} can reach node {}!", &source, &target);
                    if let Some(ids) = successful_requests.get_mut(&source) {
                        ids.push(target.clone());
                    } else {
                        successful_requests.insert(source.clone(), vec![target.clone()]);
                    }
                }
                Err(e) => {
                    log::error!("Node {} can't reach node {}: {}!", &source, &target, e);
                    if let Some(ids) = failed_requests.get_mut(&source) {
                        ids.push(target.clone());
                    } else {
                        failed_requests.insert(source.clone(), vec![target.clone()]);
                    }
                }
            }
        }
    }

    log::info!("Successful requests: {:#?}", successful_requests);

    network.shutdown();

    std::thread::sleep(Duration::from_secs(10));

    log::logger().flush();

    assert!(
        failed_requests.is_empty(),
        "Some nodes couldn't reach other nodes: {:#?}",
        failed_requests
    );
}
