use core::time::Duration;
use std::collections::{HashMap, LinkedList};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::Instant;
use tracing::{instrument, Level};

use crate::domain::{dht, NodeId, RoutingTable, ULNTable, UnderlayNeighborId};
use crate::use_cases::{
    EventHandler, FetchInjectData, InjectionMessageData, OneshotInjectMessageCallback,
    StoreInjectData, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseRuntime, UseCaseState,
};

use crate::messaging::dht::{DefaultLHTInput, FetchReqData, StoreReqData};
use crate::messaging::{Nonce, ProtocolMessage};
use crate::use_cases::distributed_hash_table_injector::DHTInjectorState::Running;
use crate::use_cases::inject_messages::errors::InjectMessageError;
use crate::use_cases::inject_messages::InjectionResult;

// TODO: what would be a sensible value here?
// TODO: random offsets for periodic restore AND collection of the hash table?
/// Default number of seconds between two attempts to restore an existing value.
pub const DEFAULT_PERIODIC_RESTORE: Duration = Duration::from_secs(60 * 60);

/// Configuration options for the [DistributedHashTableInjector].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableInjectorConfig {
    /// The duration between periodic restore operations.
    pub periodic_restore: Duration,
}

impl Default for DistributedHashTableInjectorConfig {
    /// Returns a new instance of the [DistributedHashTableInjectorConfig] initialized with the
    /// [DEFAULT_PERIODIC_RESTORE].
    fn default() -> Self {
        Self {
            periodic_restore: DEFAULT_PERIODIC_RESTORE,
        }
    }
}

/// Represents the state of the [DistributedHashTableInjector] UseCase.
///
/// # Enum Variants
///
/// - `Initialized`: Initial state.
/// - `Running(TimerId)`: State when the UseCase is running.
///   - `TimerId`: id of the periodic restore time to listen for.
/// - `Error`: Error state.
#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTInjectorState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

/// The [DistributedHashTableInjector] UseCase.
///
/// The UseCase is responsible for handling incoming [InjectMessage](UseCaseEvent::InjectMessage)
/// events by **creating** new [StoreReq](ProtocolMessage::StoreReq) and [FetchReq](ProtocolMessage::FetchReq) respectively.
/// It also listens for a response of the network and notifies the injector of the response using the provided
/// [OneshotInjectMessageCallback].
///
/// This UseCase also performs the periodic restore if requested. The duration can be configured
/// in the [DistributedHashTableInjectorConfig].
///
/// For handling incoming [StoreReq](ProtocolMessage::StoreReq) and [FetchReq](ProtocolMessage::FetchReq)
/// see the [DistributedHashTable](super::distributed_hash_table::DistributedHashTable) Use Case.
///
/// # Generics
///
/// - `C`: [UseCaseContext] in which the UseCase is running in.
#[derive(Debug)]
pub struct DistributedHashTableInjector<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    state: DHTInjectorState,
    config: DistributedHashTableInjectorConfig,
    nonces: HashMap<Nonce, (Instant, OneshotInjectMessageCallback)>,
    restore_data: LinkedList<StoreReqData<DefaultLHTInput>>,
}

impl UseCaseState for DHTInjectorState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE> {
    /// Creates a new instance of [DistributedHashTableInjector].
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration for the [DistributedHashTableInjector].
    ///
    /// # Returns
    ///
    /// A new instance of [DistributedHashTableInjector].
    pub fn new(config: DistributedHashTableInjectorConfig) -> Self {
        Self {
            _c: PhantomData,
            state: DHTInjectorState::default(),
            config,
            nonces: HashMap::default(),
            restore_data: LinkedList::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> Default for DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(DistributedHashTableInjectorConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn send_inject_result(
        &self,
        result: InjectionResult,
        callback: OneshotInjectMessageCallback,
    ) -> Result<(), InjectMessageError> {
        callback.send(result).map_err(|e| {
            log::error!(target: "distributed_hash_table_injector", "failed to send inject result: {e:#?}");

            InjectMessageError::SendResultFailed
        })
    }

    fn inform_injector_about_send_error(
        &self,
        injection_error: InjectMessageError,
        _nonce: &Nonce,
        callback: OneshotInjectMessageCallback,
    ) -> Result<(), InjectMessageError> {
        match injection_error {
            InjectMessageError::Isolated => {
                self.send_inject_result(InjectionResult::Isolated, callback)?
            }
            InjectMessageError::SendResultFailed => {
                // no information since the cause was that we couldn't reach the injector
                return Err(InjectMessageError::SendResultFailed);
            }
        }

        Ok(())
    }

    fn generate_distinct_nonce(&self) -> Nonce {
        loop {
            let nonce = Nonce::random();
            if !self.nonces.contains_key(&nonce) {
                break nonce;
            }
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for DistributedHashTableInjector<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = InjectMessageError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "distributed_hash_table_injector",
        "distributed_hash_table_injector",
        skip(self, context),
        fields(
            state = ?self.state,
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (
                UseCaseEvent::InjectMessage(
                    nonce,
                    InjectionMessageData::Store(
                        StoreInjectData {
                            handle,
                            data,
                            restore,
                        },
                        callback,
                    ),
                ),
                _,
            ) => {
                let payload = StoreReqData { handle, data };

                let nonce = nonce.unwrap_or_else(|| self.generate_distinct_nonce());

                dht::send_store_req(context, nonce.clone(), payload.clone());
                self.nonces
                    .insert(nonce.clone(), (Instant::now(), callback));
                if restore {
                    self.restore_data.push_back(payload);
                }
            }
            (
                UseCaseEvent::InjectMessage(
                    nonce,
                    InjectionMessageData::Fetch(FetchInjectData { handle }, callback),
                ),
                _,
            ) => {
                let payload = FetchReqData { handle };

                let nonce = nonce.unwrap_or_else(|| self.generate_distinct_nonce());

                if let Err(inject_err) = dht::send_fetch_req(context, nonce.clone(), payload) {
                    self.inform_injector_about_send_error(inject_err, &nonce, callback)?;
                } else {
                    self.nonces
                        .insert(nonce.clone(), (Instant::now(), callback.clone()));
                }
            }
            (UseCaseEvent::Message(message, ulnid), _) => {
                // don't react on looped requests
                match message {
                    ProtocolMessage::StoreReq(_) | ProtocolMessage::FetchReq(_) => return Ok(()),
                    _ => {}
                }

                if let Some(Some((instant, callback))) =
                    message.nonce().map(|nonce| self.nonces.remove(nonce))
                {
                    let elapsed = instant.elapsed();
                    self.send_inject_result(
                        InjectionResult::Answered((message.clone(), ulnid)),
                        callback,
                    )?;

                    log::trace!(target: "distributed_hash_table_injector", "Received response for nonce {:?} after {:?}", message.nonce(), elapsed);
                }
            }
            (UseCaseEvent::Timer(id), Running(our_id)) => {
                if &id == our_id {
                    for data in self.restore_data.iter() {
                        // TODO: make this more efficient
                        // 1.) calculate source routes only once for every handle
                        // 2.) pack all data to that node into a single request
                        dht::send_store_req(context, Nonce::random(), data.clone());
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for DistributedHashTableInjector<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = DHTInjectorState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.periodic_restore);

        self.state = Running(timer_id);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}
