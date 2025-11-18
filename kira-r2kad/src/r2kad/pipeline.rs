use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::Deref;

use derive_more::derive::{Display, Error};
use tracing::{Level, span};

use crate::context::UseCaseContext;
use crate::domain::{
    InsertionStrategy, NodeId, RoutingTable, ULNTable, UnderlayNeighborId, VicinityGraph,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::handle_api::HandleApi;
use crate::use_cases::handle_contact_update::HandleContactUpdate;
use crate::use_cases::handle_overlay_discovery::HandleOverlayDiscovery;
use crate::use_cases::{
    EventHandler,
    UseCase,
    UseCaseEvent,
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
    vicinity_discovery::VicinityDiscovery,
};
use crate::use_cases::{HandlingResult, UseCaseState};

#[derive(Debug, Clone, Copy, Default)]
pub struct R2KadPipelineConfig {
    //pub benchmark_path: Option<BufWriter<File>>,
}

// NOTE: in the future this can be refactored into a trait
//     to allow "custom" pipeline creations
#[derive(Debug)]
pub struct R2KadPipeline<C, const BUCKET_SIZE: usize> {
    derive_forwarding_tables: DeriveFwdTableEntries<C, BUCKET_SIZE>,
    distributed_hash_table: DistributedHashTable<C, DefaultExpiringHashTable, BUCKET_SIZE>,
    distributed_hash_table_injector: DistributedHashTableInjector<C, BUCKET_SIZE>,
    explicit_path_management: ExplicitPathManagement<C, BUCKET_SIZE>,
    failure_handling: FailureHandling<C, BUCKET_SIZE>,
    forward_message: ForwardProtocolMessage<C, BUCKET_SIZE>,
    handle_api: HandleApi<C>,
    contact_update: HandleContactUpdate<C, BUCKET_SIZE>,
    overlay_disc: HandleOverlayDiscovery<C, BUCKET_SIZE>,
    //inject_messages: InjectMessages<C, IRS, BUCKET_SIZE>,
    on_disc: OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>,
    path_probing: PathProbing<C, BUCKET_SIZE>,
    precomputation: PrecomputePathIds<C, BUCKET_SIZE>,
    random_probing: RandomOverlayDiscovery<C, BUCKET_SIZE>,
    vicinity_disc: VicinityDiscovery<C, BUCKET_SIZE>,
}

impl<C, const BUCKET_SIZE: usize> R2KadPipeline<C, BUCKET_SIZE> {
    pub fn new(_config: R2KadPipelineConfig) -> Self {
        let derive_forwarding_tables = DeriveFwdTableEntries::new(Default::default());
        let distributed_hash_table: DistributedHashTable<_, DefaultExpiringHashTable, BUCKET_SIZE> =
            DistributedHashTable::new(Default::default());
        let distributed_hash_table_injector = DistributedHashTableInjector::default();
        let explicit_path_management = ExplicitPathManagement::new(Default::default());
        let failure_handling = FailureHandling::new(Default::default());
        let forward_message = ForwardProtocolMessage::default();
        let handle_api = HandleApi::default();
        let contact_update = HandleContactUpdate::default();
        let overlay_disc = HandleOverlayDiscovery::default();
        let random_probing = RandomOverlayDiscovery::new(Default::default())
            .expect("default grouping should be valid");
        let on_disc = OverlayNeighborhoodDiscovery::<_, BUCKET_SIZE>::new(Default::default())
            .expect("default grouping should be valid");
        let vicinity_disc = VicinityDiscovery::default();

        let path_probing = PathProbing::new(Default::default());
        let precomputation = PrecomputePathIds::default();

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
            derive_forwarding_tables,
            distributed_hash_table,
            distributed_hash_table_injector,
            explicit_path_management,
            failure_handling,
            forward_message,
            handle_api,
            contact_update,
            overlay_disc,
            //inject_messages,
            on_disc,
            path_probing,
            precomputation,
            random_probing,
            vicinity_disc,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> Default for R2KadPipeline<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

#[derive(Debug, Display, Error)]
#[display("Some use case is in error state")]
pub struct UseCaseStateError;

#[derive(Debug, Display, Error)]
#[display("Some use case did not start up successfully")]
pub struct UseCaseStartupError;

impl<C, const BUCKET_SIZE: usize> R2KadPipeline<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>> + std::fmt::Debug,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + std::fmt::Debug,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::UnderlayNeighborTable, BUCKET_SIZE>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    pub fn startup(&mut self, context: &C) -> Result<(), UseCaseStartupError> {
        // Initialize the Use Cases

        if let Err(e) = self.forward_message.start(context) {
            log::error!("Failed to start forwarding UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.random_probing.start(context) {
            log::error!("Failed to start Random Probing UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.on_disc.start(context) {
            log::error!("Failed to start overlay neighbor discovery UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.vicinity_disc.start(context) {
            log::error!("Failed to start overlay neighbor discovery UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.path_probing.start(context) {
            log::error!("Failed to start path probing UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.derive_forwarding_tables.start(context) {
            log::error!("Failed to start path probing UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.failure_handling.start(context) {
            log::error!("Failed to start failure handling UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.precomputation.start(context) {
            log::error!("Failed to start precomputation: {e}");
            return Err(UseCaseStartupError);
        }

        if let Err(e) = self.explicit_path_management.start(context) {
            log::error!("Failed to start explicit path management: {e}");
        }

        if let Err(e) = self.distributed_hash_table.start(context) {
            log::error!("Failed to start distributed hash table UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        //    if let Err(e) = self.inject_messages.start(context) {
        //        log::error!("Failed to start inject messages UseCase: {}", e);
        //        return Err(UseCaseStartupError);
        //    }

        if let Err(e) = self.distributed_hash_table_injector.start(context) {
            log::error!("Failed to start distributed hash table injector UseCase: {e}");
            return Err(UseCaseStartupError);
        }

        Ok(())
    }

    pub fn process_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<(), UseCaseStateError> {
        let _span = match &event {
            UseCaseEvent::Message(message, _) => {
                let nonce = match message.nonce() {
                    Some(nonce) => nonce.to_string(),
                    None => "None".to_string(),
                };
                span!(target: "r2kad", Level::DEBUG, "event", "type" = "Message", %nonce, source = %message.source(), details = ?message)
            }
            UseCaseEvent::UnderlayUpdate(event) => {
                span!(target: "r2kad", Level::DEBUG, "event", "type" = "UnderlayUpdate", details = ?event)
            }
            UseCaseEvent::Contact(event) => {
                span!(target: "r2kad", Level::DEBUG, "event", "type" = "Contact", details = ?event)
            }
            UseCaseEvent::InjectMessage(nonce, _) => {
                span!(target: "r2kad", Level::DEBUG, "event", "type" = "InjectMessage", nonce = ?nonce)
            }
            UseCaseEvent::API(event) => {
                span!(target: "r2kad", Level::DEBUG, "event", "type" = "API", details = ?event)
            }
            UseCaseEvent::Vicinity(event) => {
                span!(target: "r2kad", Level::DEBUG, "event", "type" = "Vicinity", details = ?event)
            }
            UseCaseEvent::Timer(id) => {
                span!(target: "r2kad", Level::DEBUG, "event", "type" = "Timer", %id)
            }
        }
        .entered();

        // Setup paths before we forward them to setup paths on intermediate nodes too
        match self
            .explicit_path_management
            .handle_event(context, event.clone())
        {
            Err(e) => log::error!("Explicit path management returned error handling message: {e}"),
            Ok(HandlingResult::Handled) => return Ok(()), /* Skip delegation to other use cases, since path-setup/-teardown is complete */
            Ok(HandlingResult::NotHandled) => { /* Delegate event to use cases */ }
        }

        // Some precomputation to perform actions and delegate which are common tasks
        match self.forward_message.handle_event(context, event.clone()) {
            Err(e) => {
                log::error!("Forwarding protocol message returned error handling message: {e}")
            }
            Ok(HandlingResult::Handled) => return Ok(()), /* Skip delegation to other use cases */
            Ok(HandlingResult::NotHandled) => { /* Delegate event to use cases  */ }
        }
        if let Err(e) = self.overlay_disc.handle_event(context, event.clone()) {
            log::error!("Handling overlay discovery failed: {e}");
        }
        if let Err(e) = self.contact_update.handle_event(context, event.clone()) {
            log::error!("Handling contact update failed: {e}");
        }

        // Actual use cases
        if let Err(e) = self.failure_handling.handle_event(context, event.clone()) {
            log::error!("Failure handling returned error handling message: {e}");
        }
        if let Err(e) = self.random_probing.handle_event(context, event.clone()) {
            log::error!("Random Probing returned error handling message: {e}");
        }
        if let Err(e) = self.on_disc.handle_event(context, event.clone()) {
            log::error!("Overlay Neighborhood Discovery returned error handling message: {e}");
        }
        if let Err(e) = self.vicinity_disc.handle_event(context, event.clone()) {
            log::error!("Vicinity Discovery returned error handling message: {e}");
        }
        if let Err(e) = self.path_probing.handle_event(context, event.clone()) {
            log::error!("Path Probing returned error handling message: {e}");
        }
        if let Err(e) = self
            .derive_forwarding_tables
            .handle_event(context, event.clone())
        {
            log::error!("DeriveFwdEntries returned error handling message: {e}");
        }
        if self
            .precomputation
            .handle_event(context, event.clone())
            .is_err()
        {
            log::error!("Precomputation returned error handling message");
        }
        if let Err(e) = self
            .distributed_hash_table
            .handle_event(context, event.clone())
        {
            log::error!("Distributed Hash Table returned error handling message: {e}");
        }
        //if let Some(Err(e)) = self
        //    .inject_messages
        //    .as_mut()
        //    .map(|use_case| use_case.handle_event(context, event.clone()))
        //{
        //    log::error!("Injecting Messages returned error handling message: {}", e);
        //}
        if let Err(e) = self
            .distributed_hash_table_injector
            .handle_event(context, event.clone())
        {
            log::error!("Injecting DHT Messages returned error handling message: {e}");
        }

        if let Err(e) = self.handle_api.handle_event(context, event.clone()) {
            log::error!("Handling API request returned error: {e}");
        }

        // Check States as returning an error doesn't show an unrecoverable error
        let states: Vec<&(dyn UseCaseState)> = vec![
            self.forward_message.state(),
            self.failure_handling.state(),
            self.random_probing.state(),
            self.on_disc.state(),
            self.vicinity_disc.state(),
            self.derive_forwarding_tables.state(),
            self.path_probing.state(),
            self.precomputation.state(),
            self.explicit_path_management.state(),
        ];
        //// As Injection can be disabled -> Need to append.
        //if let Some(state) = inject_messages.as_ref().map(InjectMessages::state) {
        //    states.push(state);
        //}

        if states.iter().any(|use_case| use_case.is_error()) {
            log::error!("Some use case is in error state");
            Err(UseCaseStateError)
        } else {
            Ok(())
        }
    }
}
