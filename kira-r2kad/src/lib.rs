//! Source code documentation for the crate of the R²/KAD protocol implementation created at the
//! [institute for telematics](https://telematics.tm.kit.edu/index.php) at the
//! [Karlsruher Institute of Technology (KIT)](https://www.kit.edu).
//!
//! # Architecture
//!
//!  <div>
//! <img src="../../../docs/images/r2kad.svg" />
//! </div>
//!
//! # Where is the I/O?
//!
//! The protocol implementation employs the [sans I/O](https://sans-io.readthedocs.io/) design principles.
//! Therefore in this crate there is no I/O implementation present.
//! Additionally it does not include any (de)serialization of protocol messages.
//! If you enable the feature flag **`serde`** we provide generic (de)serialization support.
//!
//! The [kira-lib](../kira_lib/index.html) crate provides components on integration this R²/KAD protocol implementation
//! in a fully working KIRA instance using async futures for the different KIRA components
//! that can be run independently.
//! A fully integrated routing daemon implementation can be found in [kirad](../kirad/index.html).
//!
//! # Logging Targets
//!
//! Logging is implemented through the [log](https://crates.io/crates/log) crate.
//! While sometimes default logging targets based on module structure are used, some special
//! logging targets have been added.
//!
//! - `routing_table`: Updates to the routing table.
//! - `un_table`: Updated to the underlay neighbor table.
//! - `r2kad`: Information about the event processing by the R²/KAD instance.
//!
//! Use Case related:
//!
//! - `derive_fwd_table_entries`: Logs of the use case [DeriveFwdTableEntries](use_cases::derive_fwd_table_entries::DeriveFwdTableEntries).
//! - `explicit_path_management`: Use case [ExplicitPathManagement](use_cases::explicit_path_management::ExplicitPathManagement)
//! - `failure_handling`: Use case [FailureHandling](use_cases::failure_handling::FailureHandling)
//! - `forward_protocol_message`: Use case [ForwardProtocolMessage](use_cases::forward_protocol_message::ForwardProtocolMessage)
//! - `handle_contact_update`: Use case [HandleContactUpdate](use_cases::handle_contact_update::HandleContactUpdate)
//! - `handle_overlay_discovery`: Use case [HandleOverlayDiscovery](use_cases::handle_overlay_discovery::HandleOverlayDiscovery)
//! - `inject_messages`: Use case [InjectMessages](use_cases::inject_messages::InjectMessages)
//! - `overlay_neighborhood_discovery`: Use case [OverlayNeighborhoodDiscovery](use_cases::overlay_neighborhood_discovery::OverlayNeighborhoodDiscovery)
//! - `path_probing`: Use case [PathProbing](use_cases::path_probing::PathProbing)
//! - `precompute_paths_and_path_ids`: Use case [PathProbing](use_cases::precompute_paths_and_path_ids::PrecomputePathIds)
//! - `vicinity_discovery`: Use case [VicinityDiscovery](use_cases::vicinity_discovery::VicinityDiscovery)
//! - `distributed_hash_table`: Use cases [DistributedHashTable](use_cases::distributed_hash_table::DistributedHashTable) [DistributedHashTableInjector](use_cases::distributed_hash_table_injector::DistributedHashTableInjector)
//!
//! # Cargo feature flags
//!
//! - **`serde`**  —  Provide serialization and deserialization support using the [serde] framework.
//! - **`sha2`**  —  Provide support for generating [PathIds](crate::domain::PathId) using SHA-2.
//! - **`sha3`**  —  Provide support for generating [PathIds](crate::domain::PathId) SHA-3.

#![forbid(unsafe_code)]
//#![warn(missing_docs)]

pub mod context;
pub mod domain;
pub mod messaging;
pub mod r2kad;
pub mod runtime;
pub mod use_cases;
pub mod utils;

#[doc(inline)]
pub use crate::r2kad::{Input, Output, R2Kad};

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod tests {
    use crate::domain::NodeId;

    pub fn init() {
        // setup pretty trace logs to be captured in tests
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_test_writer()
            .pretty()
            .try_init();

        tracing::trace!("Setup collection of tracing events in test environment");
    }

    /// Returns the mapping of topo-id to [NodeId] of the [minimal topology].
    ///
    /// [minimal topology]: ../../tests/topos/minimal.gml
    pub fn minimal_topo_nodes() -> [NodeId; 20] {
        [
            NodeId::from(u128::from_str_radix("e3e7c2094cac629f6fbed82c07cd", 16).unwrap()),
            NodeId::from(u128::from_str_radix("f72842485e3a0a5d2f346baa9455", 16).unwrap()),
            NodeId::from(u128::from_str_radix("eb1167a9c3787c65c1e582e2e662", 16).unwrap()),
            NodeId::from(u128::from_str_radix("f7c14da5e709d4713d60c8a70639", 16).unwrap()),
            NodeId::from(u128::from_str_radix("e4439558867f5ba91faf7a024204", 16).unwrap()),
            NodeId::from(u128::from_str_radix("23a78133287637ebdcd9e87a1613", 16).unwrap()),
            NodeId::from(u128::from_str_radix("1846c17c627923c6612f48268673", 16).unwrap()),
            NodeId::from(u128::from_str_radix("fcbd40212ef7cca5a5a19e4d6e3c", 16).unwrap()),
            NodeId::from(u128::from_str_radix("b486fb97d43588561712e8e5216a", 16).unwrap()),
            NodeId::from(u128::from_str_radix("259fe6f4590b9a164106cf6a659e", 16).unwrap()),
            NodeId::from(u128::from_str_radix("12e0bad640fb19488dec4f65d4d9", 16).unwrap()),
            NodeId::from(u128::from_str_radix("5487af19922ad9b8a714e61a441c", 16).unwrap()),
            NodeId::from(u128::from_str_radix("5a9219c78df48f4ff31e78de5857", 16).unwrap()),
            NodeId::from(u128::from_str_radix("a3f29c6316b950f244556f25e2a2", 16).unwrap()),
            NodeId::from(u128::from_str_radix("8d72f77383c13458a748e9bb17bc", 16).unwrap()),
            NodeId::from(u128::from_str_radix("8577dd84f39e71545a137a1d5006", 16).unwrap()),
            NodeId::from(u128::from_str_radix("eb20ce164dba0ff18e0242af9fc3", 16).unwrap()),
            NodeId::from(u128::from_str_radix("17e003983ca8ea7e9d498c778ea6", 16).unwrap()),
            NodeId::from(u128::from_str_radix("b5d366194cb1d71037d1b83e90ec", 16).unwrap()),
            NodeId::from(u128::from_str_radix("a011ab0c1681c8f8e3d0d3290a4c", 16).unwrap()),
        ]
    }
}
