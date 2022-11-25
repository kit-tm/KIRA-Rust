use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::hash::Hash;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::thread::{JoinHandle, ThreadId};

use chrono::Utc;
use log::{LevelFilter, Record};
use log4rs::append::console::ConsoleAppender;
use log4rs::append::Append;
use log4rs::config::{Appender, Logger, Root};
use log4rs::Config;
use petgraph::prelude::EdgeRef;
use petgraph::{Graph, Undirected};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;

pub use cable::*;
use r2kad_daemon_lib::{Node, NodeConfig, NodeHandle};
use r2kad_lib::domain::NodeId;
use r2kad_lib::forwarding::in_memory_tables::InMemoryFwdTables;
use r2kad_lib::messaging::error::SenderError;
use r2kad_lib::messaging::{AsyncProtocolMessageReceiver, ProtocolMessage, ProtocolMessageSender};

mod cable;

/// Setup of the logging framework.
///
/// This creates a folder structure `log/integration_test/{test_name}` which contains the logs
/// grouped by node id of the node which is running in the corresponding thread.
/// The default log has the node id `0000000000000000000000000000` and is located in the file
/// `0000000000.log`.
pub fn setup(test_name: &'static str) {
    let logs_path = PathBuf::new()
        .join("log")
        .join("integration_test")
        .join(test_name);
    let _ = std::fs::remove_dir_all(logs_path.clone());
    std::fs::create_dir_all(logs_path.clone()).expect("failed to create logs dir");

    let stdout = ConsoleAppender::builder().build();

    let files = ThreadSplitAppender {
        root_dir: logs_path,
        files: Arc::new(Default::default()),
    };

    let config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .appender(Appender::builder().build("files", Box::new(files)))
        .logger(
            Logger::builder()
                .appender("stdout")
                .additive(true)
                .build(test_name, LevelFilter::Trace),
        )
        .build(Root::builder().appender("files").build(LevelFilter::Debug))
        .unwrap();

    let _ = log4rs::init_config(config);
}

#[derive(Debug)]
struct ThreadSplitAppender {
    root_dir: PathBuf,
    files: Arc<std::sync::Mutex<HashMap<ThreadId, BufWriter<File>>>>,
}

impl ThreadSplitAppender {
    fn get_node_id(&self, thread_id: &ThreadId) -> NodeId {
        std::env::var(format!("{:?}_NODE_ID", thread_id))
            .map(|val| NodeId::from_str(&val).expect("invalid node id in env var"))
            .unwrap_or_else(|_| NodeId::zero())
    }

    fn create_new_file(&self, thread_id: ThreadId) -> anyhow::Result<()> {
        let node_id = self.get_node_id(&thread_id);
        let path = format!("{:.10}.log", node_id);
        let new_file_path = self.root_dir.join(path);
        let new_file = BufWriter::new(File::create(new_file_path)?);
        let mut files = self.files.lock().unwrap();
        files.insert(thread_id, new_file);
        Ok(())
    }
}

impl Append for ThreadSplitAppender {
    fn append(&self, record: &Record) -> anyhow::Result<()> {
        let thread = std::thread::current().id();

        if !self.files.lock().unwrap().contains_key(&thread) {
            self.create_new_file(thread)?;
        }
        let node_id = self.get_node_id(&thread);

        let mut files = self.files.lock().unwrap();
        let files = files.get_mut(&thread).unwrap(); // Checked before;

        writeln!(
            files,
            "{} {} {:.10} {} - {}",
            Utc::now().to_rfc3339(),
            record.level(),
            node_id,
            record.target(),
            record.args()
        )?;

        Ok(())
    }

    fn flush(&self) {
        let mut files = self.files.lock().unwrap();
        for (_, file) in files.iter_mut() {
            let _ = file.flush();
        }
    }
}

/// Implements an unordered pair identifying a link.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct LinkIdx(NodeId, NodeId);

impl From<(NodeId, NodeId)> for LinkIdx {
    fn from((first, second): (NodeId, NodeId)) -> Self {
        if first < second {
            LinkIdx(first, second)
        } else {
            LinkIdx(second, first)
        }
    }
}

type NodesMap = HashMap<
    NodeId,
    (
        Node<IdDelegator<CloseableSender>, InMemoryFwdTables>,
        Sender<Box<dyn AsyncProtocolMessageReceiver + Send>>,
    ),
>;

/// Represents the integration test network of nodes connected through [Cables](Cable).
pub struct Network {
    links: HashMap<LinkIdx, Cable>,
    nodes: NodesMap,
}

impl Network {
    pub fn start(mut self) -> NetworkHandle {
        let handles = self
            .nodes
            .drain()
            .map(|(id, (node, sender))| {
                let (node_handle, join_handle) = node.start();
                (id, (node_handle, Some(join_handle), sender))
            })
            .collect::<HashMap<_, _>>();

        NetworkHandle {
            links: self.links,
            nodes: handles,
        }
    }
}

type NodesHandleMap = HashMap<
    NodeId,
    (
        NodeHandle,
        Option<JoinHandle<()>>,
        Sender<Box<dyn AsyncProtocolMessageReceiver + Send>>,
    ),
>;

pub struct NetworkHandle {
    links: HashMap<LinkIdx, Cable>,
    nodes: NodesHandleMap,
}

impl NetworkHandle {
    pub fn link(&self, idx: &LinkIdx) -> Option<&Cable> {
        self.links.get(idx)
    }

    pub fn link_mut(&mut self, idx: &LinkIdx) -> Option<&mut Cable> {
        self.links.get_mut(idx)
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeHandle> {
        self.nodes.get(id).map(|(handle, _, _)| handle)
    }

    pub fn node_mut(&mut self, id: &NodeId) -> Option<&mut NodeHandle> {
        self.nodes.get_mut(id).map(|(handle, _, _)| handle)
    }

    pub fn shutdown(&mut self) {
        for (handle, join_handle, _) in self.nodes.values() {
            if join_handle.is_some() {
                handle.shutdown();
            }
        }
    }
}

impl<E> From<Graph<NodeId, E, Undirected>> for Network {
    fn from(graph: Graph<NodeId, E, Undirected>) -> Self {
        let mut links = HashMap::with_capacity(graph.edge_count());
        let mut nodes = HashMap::with_capacity(graph.node_count());

        for edge in graph.edge_indices() {
            let (source, target) = graph.edge_endpoints(edge).unwrap();
            let edge_index = LinkIdx::from((
                graph.node_weight(source).cloned().unwrap(),
                graph.node_weight(target).cloned().unwrap(),
            ));

            links
                .entry(edge_index.clone())
                .or_insert_with(|| Cable::new(edge_index.0.clone(), edge_index.1.clone()));
        }

        for node_index in graph.node_indices() {
            // Using one runtime per node
            let runtime = Arc::new(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime"),
            );

            let node_id = graph.node_weight(node_index);
            assert!(node_id.is_some());
            let node_id = node_id.unwrap();

            let (neighbor_senders, neighbor_receivers): (
                _,
                Vec<Box<dyn AsyncProtocolMessageReceiver + Send>>,
            ) = graph
                .edges(node_index)
                .map(|edge| {
                    let neighbor_id = if graph.node_weight(edge.source()).unwrap() == node_id {
                        graph.node_weight(edge.target()).unwrap()
                    } else {
                        graph.node_weight(edge.source()).unwrap()
                    };
                    let edge_idx = LinkIdx::from((node_id.clone(), neighbor_id.clone()));
                    let opt_edge = links.get_mut(&edge_idx);
                    if opt_edge.is_none() {
                        panic!("Link {:?} not created yet", edge_idx);
                    }
                    let link_hub = opt_edge.unwrap().take_parts_for(node_id);
                    assert!(link_hub.is_some(), "Cable already consumed for {}", node_id);
                    let (sender, receiver) = link_hub.unwrap();
                    (neighbor_id.clone(), sender, receiver)
                })
                .fold(
                    (HashMap::new(), Vec::new()),
                    |(mut senders, mut receivers), (neighbor_id, sender, receiver)| {
                        senders.insert(neighbor_id, sender);
                        receivers.push(Box::new(receiver));

                        (senders, receivers)
                    },
                );

            let (neighbor_receivers_sender, neighbor_receivers_receiver) =
                mpsc::channel(neighbor_receivers.len() + 1);
            for receiver in neighbor_receivers {
                if neighbor_receivers_sender.blocking_send(receiver).is_err() {
                    panic!("Failed to send receiver to send receiver");
                }
            }

            let node = Node::new(
                NodeConfig {
                    heuristic_enabled: false,
                    benchmark_path: None,
                },
                node_id.clone(),
                Arc::clone(&runtime),
                neighbor_receivers_receiver,
                IdDelegator {
                    neighbor_links: neighbor_senders,
                },
                InMemoryFwdTables::new(),
            );

            nodes.insert(node_id.clone(), (node, neighbor_receivers_sender));
        }

        Network { links, nodes }
    }
}

/// Delegates messages based on neighbors id in message.
///
/// Construction of this type is only allowed for [Network].
///
/// Silently drops messages if sent to invalid neighbors.
#[derive(Debug)]
pub struct IdDelegator<S: Debug> {
    neighbor_links: HashMap<NodeId, S>,
}

impl<S> ProtocolMessageSender for IdDelegator<S>
where
    S: ProtocolMessageSender + Debug,
{
    fn send_message<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage>,
    {
        let message = message.into();

        match message
            .current_hop()
            .filter(|id| self.neighbor_links.contains_key(id))
            .cloned()
        {
            Some(id) => {
                let link = self
                    .neighbor_links
                    .get_mut(&id)
                    .expect("trying to send to non existent neighbor");
                if let Err(SenderError::Closed) = link.send_message(message) {
                    self.neighbor_links.remove(&id);
                    log::trace!("Removed sender for {} from delegation.", id);
                }
            }
            _ => {
                for link in self.neighbor_links.values_mut() {
                    let _ = link.send_message(message.clone());
                }
            }
        }

        Ok(())
    }
}
