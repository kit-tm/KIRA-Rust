use std::time::Instant;

use super::Result;
use crate::domain::NodeId;
use crate::use_cases::{
    derive_fwd_table_entries::DeriveFwdTableEntries,
    distributed_hash_table::{DefaultExpiringHashTable, DistributedHashTable},
    distributed_hash_table_injector::DistributedHashTableInjector,
    explicit_path_management::ExplicitPathManagement,
    failure_handling::FailureHandling,
    forward_protocol_message::ForwardProtocolMessage,
    //inject_messages::InjectMessages,
    overlay_neighborhood_discovery::OverlayNeighborhoodDiscovery,
    path_probing::PathProbing,
    precompute_paths_and_path_ids::PrecomputePathIds,
    random_overlay_discovery::RandomOverlayDiscovery,
    vicinity_discovery::{VicinityDiscovery, VicinityDiscoveryConfig},
    UseCaseEvent,
};

#[derive(Debug, Clone, Default)]
pub struct R2KadPipelineConfig {
    pub heuristic_enabled: bool,
    //pub benchmark_path: Option<BufWriter<File>>,
}

// NOTE: in the future this can be refactored into a trait
//     to allow "custom" pipeline creations
pub struct R2KadPipeline<C, const BUCKET_SIZE: usize> {
    forward_message: ForwardProtocolMessage<C, BUCKET_SIZE>,
    random_probing: RandomOverlayDiscovery<C, BUCKET_SIZE>,
    on_disc: OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>,
    vicinity_disc: VicinityDiscovery<C, BUCKET_SIZE>,
    path_probing: PathProbing<C, BUCKET_SIZE>,
    derive_forwarding_tables: DeriveFwdTableEntries<C, BUCKET_SIZE>,
    failure_handling: FailureHandling<C, BUCKET_SIZE>,
    precomputation: PrecomputePathIds<C, BUCKET_SIZE>,
    explicit_path_management: ExplicitPathManagement<C, BUCKET_SIZE>,
    distributed_hash_table: DistributedHashTable<C, DefaultExpiringHashTable, BUCKET_SIZE>,
    distributed_hash_table_injector: DistributedHashTableInjector<C, BUCKET_SIZE>,
    //inject_messages: InjectMessages<C, IRS, BUCKET_SIZE>,
}

impl<C, const BUCKET_SIZE: usize> R2KadPipeline<C, BUCKET_SIZE> {
    pub fn new(config: R2KadPipelineConfig, root_id: &NodeId) -> Self {
        let mut forward_message = ForwardProtocolMessage::default();
        let mut random_probing = RandomOverlayDiscovery::new(Default::default())
            .expect("default grouping should be valid");
        let mut on_disc = OverlayNeighborhoodDiscovery::<_, BUCKET_SIZE>::new(Default::default())
            .expect("default grouping should be valid");

        let vicinity_config = VicinityDiscoveryConfig {
            heuristic_enabled: config.heuristic_enabled,
            ..Default::default()
        };
        let mut vicinity_disc = VicinityDiscovery::new(vicinity_config);

        let mut path_probing = PathProbing::new(Default::default());
        let mut derive_forwarding_tables = DeriveFwdTableEntries::new(Default::default());
        let mut failure_handling = FailureHandling::new(Default::default());
        let mut precomputation = PrecomputePathIds::new(root_id.clone(), Default::default());
        let mut explicit_path_management = ExplicitPathManagement::new(Default::default());
        let mut distributed_hash_table: DistributedHashTable<
            _,
            DefaultExpiringHashTable,
            BUCKET_SIZE,
        > = DistributedHashTable::new(Default::default());
        let mut distributed_hash_table_injector = DistributedHashTableInjector::default();

        //let mut inject_messages = if let Some(injection_sender) = injection_sender.as_ref() {
        //    let mut inject_messages =
        //        InjectMessages::new(Default::default(), injection_sender.clone())
        //            .expect("default grouping should be valid");
        //    if let Err(e) = inject_messages.start(&context) {
        //        log::error!("Failed to start inject messages UseCase: {}", e);
        //        return;
        //    }
        //    Some(inject_messages)
        //} else {
        //    None
        //};

        Self {
            forward_message,
            random_probing,
            on_disc,
            vicinity_disc,
            path_probing,
            derive_forwarding_tables,
            failure_handling,
            precomputation,
            explicit_path_management,
            distributed_hash_table,
            distributed_hash_table_injector,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> R2KadPipeline<C, BUCKET_SIZE> {
    pub fn startup(&mut self, context: &C, now: Instant) -> Result<()> {
        todo!("Implement R2KadPipeline::startup");
    }

    pub fn process_event(&mut self, context: &C, event: UseCaseEvent) -> Result<()> {
        todo!("Implement R2KadPipeline::process_event");
    }
}
