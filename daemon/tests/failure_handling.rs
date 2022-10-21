use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::time::Duration;

use petgraph::dot::{Config, Dot};
use petgraph::prelude::*;

use r2kad_lib::domain::NodeId;
use r2kad_lib::messaging::{FindNodeReqData, ProtocolMessage};

use crate::common::{LinkIdx, Network};

mod common;

#[test]
#[ntest::timeout(300000)]
fn failure_handling() {
    common::setup("failure_handling");

    /*
    Topology:
        root_id: 1 (index: 0)

         /- 2 -\
       1 -- 3 -- 4 -- 5
            |         |
         `- 6 -- 7 -´

    */

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
    ];

    for node_id in &node_ids {
        graph.add_node(node_id.clone());
    }

    // 1 -- 2
    graph.add_edge(NodeIndex::new(0), NodeIndex::new(1), ());
    // 1 -- 3
    graph.add_edge(NodeIndex::new(0), NodeIndex::new(2), ());
    // 1 -- 6
    graph.add_edge(NodeIndex::new(0), NodeIndex::new(5), ());
    // 2 -- 4
    graph.add_edge(NodeIndex::new(1), NodeIndex::new(3), ());
    // 3 -- 4
    graph.add_edge(NodeIndex::new(2), NodeIndex::new(3), ());
    // 3 -- 6
    graph.add_edge(NodeIndex::new(2), NodeIndex::new(5), ());
    // 4 -- 5
    graph.add_edge(NodeIndex::new(3), NodeIndex::new(4), ());
    // 5 -- 7
    graph.add_edge(NodeIndex::new(4), NodeIndex::new(6), ());
    // 6 -- 7
    graph.add_edge(NodeIndex::new(5), NodeIndex::new(6), ());

    println!(
        "Using network: {:?}",
        Dot::with_config(&graph, &[Config::EdgeNoLabel])
    );

    let network = Network::from(graph);

    let mut network = network.start();

    // Wait for stable state
    std::thread::sleep(Duration::from_secs(20));

    // Close all channels to node 3 and 6
    // 3 -- 1
    network
        .link_mut(&LinkIdx::from((node_ids[2].clone(), node_ids[0].clone())))
        .expect("link between node 3 and node 1 doesn't exist")
        .blocking_close();
    // 3 -- 4
    network
        .link_mut(&LinkIdx::from((node_ids[2].clone(), node_ids[3].clone())))
        .expect("link between node 3 and node 4 doesn't exist")
        .blocking_close();
    // 3 -- 6
    network
        .link_mut(&LinkIdx::from((node_ids[2].clone(), node_ids[5].clone())))
        .expect("link between node 3 and node 6 doesn't exist")
        .blocking_close();
    // 6 -- 1
    network
        .link_mut(&LinkIdx::from((node_ids[5].clone(), node_ids[0].clone())))
        .expect("link between node 6 and node 1 doesn't exist")
        .blocking_close();
    // 6 -- 7
    network
        .link_mut(&LinkIdx::from((node_ids[5].clone(), node_ids[6].clone())))
        .expect("link between node 6 and node 7 doesn't exist")
        .blocking_close();

    // Wait for the rediscovery processes to finish
    std::thread::sleep(Duration::from_secs(20));

    // Test reachability after outage
    // Tests if all non-isolated nodes still reach each other
    // Tests if all isolated nodes can't reach any other node
    let mut failed_requests: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();
    let mut successful_requests: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();

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
                    if !matches!(message, ProtocolMessage::FindNodeRsp(_)) {
                        log::debug!(
                            "Received response which is not a FindNodeRsp: {:?}",
                            message
                        );
                        if let Some(ids) = failed_requests.get_mut(&source) {
                            ids.insert(target.clone());
                        } else {
                            failed_requests.insert(source.clone(), HashSet::from([target.clone()]));
                        }
                    }

                    log::info!("Node {} can reach node {}!", &source, &target);
                    if let Some(ids) = successful_requests.get_mut(&source) {
                        ids.insert(target.clone());
                    } else {
                        successful_requests.insert(source.clone(), HashSet::from([target.clone()]));
                    }
                }
                Err(e) => {
                    log::info!("Node {} can't reach node {}: {}!", &source, &target, e);
                    if let Some(ids) = failed_requests.get_mut(&source) {
                        ids.insert(target.clone());
                    } else {
                        failed_requests.insert(source.clone(), HashSet::from([target.clone()]));
                    }
                }
            }
        }
    }

    log::info!("Successful requests: {:#?}", successful_requests);

    if let Some(not_reached_by_3) = failed_requests.remove(&node_ids[2]) {
        let expected = HashSet::from([
            node_ids[0].clone(),
            node_ids[1].clone(),
            node_ids[3].clone(),
            node_ids[4].clone(),
            node_ids[5].clone(),
            node_ids[6].clone(),
        ]);
        assert_eq!(
            not_reached_by_3, expected,
            "Node 3 did reach some nodes it shouldn't"
        );
    }

    if let Some(not_reached_by_6) = failed_requests.remove(&node_ids[5]) {
        let expected = HashSet::from([
            node_ids[0].clone(),
            node_ids[1].clone(),
            node_ids[2].clone(),
            node_ids[3].clone(),
            node_ids[4].clone(),
            node_ids[6].clone(),
        ]);
        assert_eq!(
            not_reached_by_6, expected,
            "Node 6 did reach some nodes it shouldn't"
        );
    }

    // Test if all other nodes only can't reach node 3 and node 6
    if let Some(not_reached_by_1) = failed_requests.get_mut(&node_ids[0]) {
        not_reached_by_1.remove(&node_ids[2]);
        not_reached_by_1.remove(&node_ids[5]);
    }
    if let Some(not_reached_by_2) = failed_requests.get_mut(&node_ids[1]) {
        not_reached_by_2.remove(&node_ids[2]);
        not_reached_by_2.remove(&node_ids[5]);
    }
    if let Some(not_reached_by_4) = failed_requests.get_mut(&node_ids[3]) {
        not_reached_by_4.remove(&node_ids[2]);
        not_reached_by_4.remove(&node_ids[5]);
    }
    if let Some(not_reached_by_4) = failed_requests.get_mut(&node_ids[4]) {
        not_reached_by_4.remove(&node_ids[2]);
        not_reached_by_4.remove(&node_ids[5]);
    }
    if let Some(not_reached_by_6) = failed_requests.get_mut(&node_ids[6]) {
        not_reached_by_6.remove(&node_ids[2]);
        not_reached_by_6.remove(&node_ids[5]);
    }
    for key in failed_requests.clone().keys() {
        if let Some(true) = failed_requests
            .get(key)
            .map(|failed_requests| failed_requests.is_empty())
        {
            failed_requests.remove(key);
        }
    }

    assert!(
        failed_requests.is_empty(),
        "Some requests failed which have not been expected: {:#?}",
        failed_requests
    );
}
