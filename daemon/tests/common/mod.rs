use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::ops::BitXor;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::thread::ThreadId;

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

use r2kad_daemon_lib::{Node, NodeConfig, NodeHandle};
use r2kad_lib::domain::{NetworkInterface, NodeId};
use r2kad_lib::forwarding::in_memory_tables::InMemoryFwdTables;
use r2kad_lib::messaging::error::SenderError;
use r2kad_lib::messaging::{
    AsyncProtocolMessageReceiver, InMemoryMessageChannel, InMemoryReceiver, InMemorySender,
    ProtocolMessage, ProtocolMessageSender,
};

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
                .build(test_name, LevelFilter::Warn),
        )
        .build(Root::builder().appender("files").build(LevelFilter::Trace))
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

#[derive(Debug, Clone)]
pub struct LinkIdx(NodeId, NodeId);

impl From<(NodeId, NodeId)> for LinkIdx {
    fn from((first, second): (NodeId, NodeId)) -> Self {
        LinkIdx(first, second)
    }
}

impl Hash for LinkIdx {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let xor = (&self.0).bitxor(&self.1);
        xor.hash(state)
    }
}

impl PartialEq<Self> for LinkIdx {
    fn eq(&self, other: &Self) -> bool {
        (self.0 == other.0 && self.1 == other.1) || (self.0 == other.1 && self.1 == other.0)
    }
}

impl Eq for LinkIdx {}

/// Link which connects two nodes.
#[derive(Debug)]
pub struct Cable {
    one: NodeId,
    two: NodeId,
    // Cables are used to distinguish between the communication direction
    // Node one has cable_one as receiver and cable_two as sender
    endpoint_one: Option<(InMemorySender, InMemoryReceiver)>,
    endpoint_two: Option<(InMemorySender, InMemoryReceiver)>,
}

impl Cable {
    fn new(one: NodeId, two: NodeId) -> Self {
        let interface_one = NetworkInterface::new(format!("{}--{}", one, two));
        let interface_two = NetworkInterface::new(format!("{}--{}", two, one));
        let (cable_one_sender, cable_one_receiver) =
            InMemoryMessageChannel::with_interface(interface_one).into_parts();
        let (cable_two_sender, cable_two_receiver) =
            InMemoryMessageChannel::with_interface(interface_two).into_parts();
        Self {
            one,
            two,
            endpoint_one: Some((cable_one_sender, cable_two_receiver)),
            endpoint_two: Some((cable_two_sender, cable_one_receiver)),
        }
    }

    fn take_parts_for(&mut self, id: &NodeId) -> Option<(InMemorySender, InMemoryReceiver)> {
        match (id == &self.one, id == &self.two) {
            (true, _) => self.endpoint_one.take(),
            (_, true) => self.endpoint_two.take(),
            _ => None,
        }
    }
}

/// Represents the integration test network of nodes connected through links/cables.
pub struct Network {
    links: HashMap<LinkIdx, Cable>,
    nodes: HashMap<
        NodeId,
        (
            Node<IdDelegator<InMemorySender>, InMemoryFwdTables>,
            Sender<Box<dyn AsyncProtocolMessageReceiver + Send>>,
        ),
    >,
}

impl Network {
    pub fn start(mut self) -> NetworkHandle {
        let handles = self
            .nodes
            .drain()
            .map(|(id, (node, sender))| (id, (node.start(), sender)))
            .collect::<HashMap<_, _>>();

        NetworkHandle {
            links: self.links,
            nodes: handles,
        }
    }
}

pub struct NetworkHandle {
    links: HashMap<LinkIdx, Cable>,
    nodes: HashMap<
        NodeId,
        (
            NodeHandle,
            Sender<Box<dyn AsyncProtocolMessageReceiver + Send>>,
        ),
    >,
}

impl NetworkHandle {
    pub fn link<Idx: AsRef<LinkIdx>>(&self, idx: Idx) -> Option<&Cable> {
        self.links.get(idx.as_ref())
    }

    pub fn link_mut<Idx: AsRef<LinkIdx>>(&mut self, idx: Idx) -> Option<&mut Cable> {
        self.links.get_mut(idx.as_ref())
    }

    pub fn node(&self, id: &NodeId) -> Option<&NodeHandle> {
        self.nodes.get(id).map(|(handle, _)| handle)
    }

    pub fn node_mut(&mut self, id: &NodeId) -> Option<&mut NodeHandle> {
        self.nodes.get_mut(id).map(|(handle, _)| handle)
    }
}

impl<E> From<Graph<NodeId, E, Undirected>> for Network {
    fn from(graph: Graph<NodeId, E, Undirected>) -> Self {
        let mut links = HashMap::with_capacity(graph.edge_count());
        let mut nodes = HashMap::with_capacity(graph.node_count());

        for edge in graph.edge_indices() {
            let (source, target) = graph.edge_endpoints(edge).unwrap();
            let edge_index = LinkIdx(
                graph.node_weight(source).cloned().unwrap(),
                graph.node_weight(target).cloned().unwrap(),
            );

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
                    let edge_idx = LinkIdx(node_id.clone(), neighbor_id.clone());
                    let link_hub = links
                        .get_mut(&edge_idx)
                        .expect("links already created")
                        .take_parts_for(node_id);
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
                neighbor_receivers_sender
                    .blocking_send(receiver)
                    .expect("failed to send receiver through channel");
            }

            let node = Node::new(
                NodeConfig {
                    message_injection_enabled: true,
                    heuristic_enabled: false,
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

        match message.current_hop().map(|id| {
            self.neighbor_links
                .get_mut(id)
                .expect("trying to send to non existent neighbor")
        }) {
            Some(link) => link.send_message(message)?,
            _ => {
                for link in self.neighbor_links.values_mut() {
                    link.send_message(message.clone())?;
                }
            }
        }

        Ok(())
    }
}
