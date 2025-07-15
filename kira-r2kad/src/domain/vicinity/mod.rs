//! Working with the vicinity of the node.

pub mod vicinity_graph;
pub use vicinity_graph::VicinityGraph;

/// Radius of the neighborhood considered as vicinity.
///
/// A radius of **3** means:
///
/// - Nodes with a distance **<= 3** hops are known to the node.
///   The node _may_ decide to add them into their routing table[^uln] as contacts.
/// - Nodes with a distance **< 3** hops are in the *vicinity*.
/// - _All_ paths to nodes[^contacts] in the vicinity are computed and installed as Fast-Forwarding.
/// - Nodes in the vicinity apart from underlay neighbors receive `QueryRouteReq`s.
/// - Underlay neighbors with a distance of **1** hops with
///   which the following messages are exchanged: `ULNHello`, `ULNDiscReq`, `ULNDiscRsp`.
///
/// An in-depth explanation can be found in the respective [KIRA-Draft section][kira_vg].
///
/// [kira_vg]: https://www.ietf.org/archive/id/draft-bless-rtgwg-kira-03.html#section-3.4-3
/// [^uln]: Underlay neighbors are _always_ included in the Routing Table.
/// [^contacts]: Even if they are not Contacts in the Routing Table. Therefor nodes in the vicinity
/// need to be kept in a separat data structure called *Vicinity Graph*.
pub const VICINITY_RADIUS: usize = 3;
