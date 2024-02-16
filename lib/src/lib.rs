//! Source code documentation for the library crate of the R²/Kad implementation created at the
//! [institute for telematics](https://telematics.tm.kit.edu/index.php) at the
//! [Karlsruher Institute of Technology (KIT)](https://www.kit.edu).
//!
//! # Logging Targets
//!
//! Logging is implemented through the [log](https://crates.io/crates/log) crate.
//! While sometimes default logging targets based on module structure are used, some special
//! logging targets have been added.
//!
//! - `routing_table`: Updates to the routing table.
//! - `pn_table`: Updated to the physical neighbor table.
//! - `message_sender`: Information about sending protocol messages.
//! - `message_receiver`: Information about receiving protocol messages.
//! - `network_interfaces`: Logging of updates to the network interfaces.
//! - `in_memory_fwd_table`: Updates to the stub implementation of the forwarding tables [InMemoryFwdTable](forwarding::in_memory_tables::InMemoryFwdTables)
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
//! - `random_overlay_discovery`: Use case [RandomOverlayDiscovery](use_cases::random_overlay_discovery::RandomOverlayDiscovery)
//! - `vicinity_discovery`: Use case [VicinityDiscovery](use_cases::vicinity_discovery::VicinityDiscovery)
//! - `distributed_hash_table`: Use case [DistributedHashTable](use_cases::distributed_hash_table::DistributedHashTable)
//! - `distributed_hash_table_injector`: Use case [DistributedHashTableInjector](use_cases::distributed_hash_table_injector::DistributedHashTableInjector)
//!
//! # Authors
//!
//! - Moritz Hepp (former student at KIT)

pub mod broadcaster;
pub mod context;
pub mod domain;
pub mod forwarding;
pub mod hardware_events;
pub mod messaging;
#[cfg(feature = "pnet")]
pub mod pnet_interface_monitor;
pub mod runtime;
pub mod use_cases;
pub mod utils;

#[cfg(test)]
pub(crate) mod tests {
    use log::LevelFilter;

    pub fn init() {
        let _ = env_logger::builder()
            .filter_level(LevelFilter::Trace)
            .parse_default_env()
            .is_test(true)
            .try_init();
    }
}
