use std::collections::HashMap;
use std::marker::PhantomData;
use std::num::{NonZeroU64, NonZeroUsize};
use std::ops::Deref;
use std::time::Duration;

use derive_more::derive::{Display, Error};

use crate::domain::{node_id, GroupingError, NodeId, RoutingTable, UNTable, UnderlayNeighborId};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{FindNodeReqData, Nonce, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    EventHandler, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseState,
};

#[derive(Debug, Copy, Clone)]
pub struct RODConfig {
    pub timeout: Duration,
    pub neighborhood_size: NonZeroU64,
    pub shared_prefix_grouping: NonZeroUsize,
}

impl Default for RODConfig {
    fn default() -> Self {
        Self {
            // Default: 2.5 Messages/s => 1000 ms / 2.5 = 400 ms
            timeout: Duration::from_millis(400),
            neighborhood_size: NonZeroU64::new(20).unwrap(),
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
        }
    }
}

/// Probe a random [NodeId] to keep [Buckets](crate::domain::Bucket) up-to-date.
#[derive(Debug)]
pub struct RandomOverlayDiscovery<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    config: RODConfig,
    state: RODState,
}

impl<C, const BUCKET_SIZE: usize> RandomOverlayDiscovery<C, BUCKET_SIZE> {
    pub fn new(config: RODConfig) -> Result<Self, GroupingError> {
        if config.shared_prefix_grouping.get() > node_id::BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_grouping.get(),
                id_size: node_id::BIT_SIZE,
            });
        }

        Ok(Self {
            _c: PhantomData,
            config,
            state: RODState::Initialized,
        })
    }
}

impl<C, const BUCKET_SIZE: usize> RandomOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn send_find_node_req(&mut self, context: &C) -> Result<(), RODError> {
        let random_id = NodeId::random();

        let closest_path = context
            .routing_table()
            .closest(
                &random_id,
                BUCKET_SIZE,
                self.config.shared_prefix_grouping.get(),
            )
            .expect("grouping was checked on initialization")
            .first()
            .map(|(_, contact)| contact.path())
            .cloned();
        if closest_path.is_none() {
            log::trace!(
                target: "random_overlay_discovery",
                "No closest contact found for random id; Assuming isolation"
            );
            return Ok(());
        }
        let closest_path = closest_path.unwrap();

        // Get interface of neighbor
        let interface = context.pn_table().get(closest_path.first()).cloned();
        if interface.is_none() {
            log::error!(
                target: "random_overlay_discovery",
                "Contacts path contains invalid neighbor: {}",
                closest_path
            );
            self.state = RODState::Error;
            return Err(RODError::InvalidNeighbor);
        }

        let mut route = SourceRoute::from(closest_path);
        route.push_front(*context.root_id());

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: FindNodeReqData {
                exact: false,
                neighborhood: self.config.neighborhood_size,
                target: random_id,
            },
            not_via: context.not_via().clone(),
            source_route: route,
        };
        log::trace!(target: "random_overlay_discovery", "Sending message {:?}", message);

        context
            .runtime()
            .send_message(message, context.pn_table().deref());

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for RandomOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = RODState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.timeout);

        self.state = RODState::Running(timer_id);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for RandomOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = RODError;
    type Value = ();

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(event_id), RODState::Running(timer_id)) =
            (event, &mut self.state)
        {
            if &event_id != timer_id {
                return Ok(());
            }

            self.send_find_node_req(context)?;
        }

        Ok(())
    }
}

#[derive(Debug, Display, Error)]
pub enum RODError {
    #[display("Could not probe, routing table is empty")]
    EmptyRoutingTable,
    #[display("The path of a contact contains an invalid neighbor")]
    InvalidNeighbor,
}

#[derive(Debug, Eq, PartialEq)]
pub enum RODState {
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for RODState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}
