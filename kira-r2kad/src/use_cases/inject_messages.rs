use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::ops::Deref;
use std::time::Instant;
use tracing::{Level, instrument};

use crate::domain::{
    GroupingError, NodeId, RoutingTable, ULNTable, UnderlayNeighborId, UnderlayNeighborSource,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{Nonce, CommonHeader, ProtocolMessageKind, ProtocolMessage, ReqRspMessage};
use crate::use_cases::inject_messages::errors::InjectMessageError;
use crate::use_cases::{
    EventHandler, InjectionMessageData, ReactiveUseCaseState, UseCase, UseCaseContext,
    UseCaseEvent, UseCaseRuntime,
};

/// Sender for [InjectionResult]s.
///
/// Used to send results for injected messages back to the outside (main component).
pub trait InjectionResultSender: Clone {
    type Error: Error;

    fn send_result(&self, result: InjectionResult) -> Result<(), Self::Error>;
}

mod std_extension {
    use std::sync::mpsc;

    use crate::use_cases::inject_messages::{InjectionResult, InjectionResultSender};

    impl InjectionResultSender for mpsc::Sender<InjectionResult> {
        type Error = mpsc::SendError<InjectionResult>;

        fn send_result(&self, result: InjectionResult) -> Result<(), Self::Error> {
            self.send(result)
        }
    }
}

/// Result yielded for an injected [ProtocolMessage].
///
/// This is sent to signal that an error occurred or the [ProtocolMessage] was answered correctly.
#[derive(Debug, Eq, PartialEq)]
pub enum InjectionResult {
    /// The injected [ProtocolMessage] was answered.
    Answered((ProtocolMessage, UnderlayNeighborSource)),
    /// The node is isolated and the message couldn't be injected.
    Isolated,
}

/// Configuration for message injection in general.
#[derive(Debug)]
pub struct InjectMessagesConfig {
    /// Grouping to use.
    ///
    /// Will be used when a FindNodeReq is injected and the closest node needs to be found.
    pub shared_prefix_grouping: NonZeroU8,
}

impl Default for InjectMessagesConfig {
    fn default() -> Self {
        Self {
            shared_prefix_grouping: NonZeroU8::new(1).unwrap(),
        }
    }
}

/// [UseCase] for externals to inject messages into the network.
///
/// Will yield results through the given [InjectionResultSender] (`TS`).
#[derive(Debug)]
pub struct InjectMessages<C, IRS, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    config: InjectMessagesConfig,
    state: ReactiveUseCaseState,
    injection_result_sender: IRS,
    nonces: HashMap<Nonce, Instant>,
}

impl<C, TS, const BUCKET_SIZE: usize> InjectMessages<C, TS, BUCKET_SIZE> {
    /// Creates a new [InjectMessages] instance and checks the grouping before.
    pub fn new(
        config: InjectMessagesConfig,
        sender: TS,
    ) -> Result<InjectMessages<C, TS, BUCKET_SIZE>, GroupingError> {
        if config.shared_prefix_grouping.get() > NodeId::BITS {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_grouping,
            });
        }

        let use_case = Self {
            _pd: Default::default(),
            config,
            state: ReactiveUseCaseState::default(),
            injection_result_sender: sender,
            nonces: HashMap::new(),
        };

        Ok(use_case)
    }
}

impl<C, TS, const BUCKET_SIZE: usize> EventHandler for InjectMessages<C, TS, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    TS: InjectionResultSender,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = InjectMessageError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "inject_messages",
        "inject_messages",
        skip(self, context),
        fields(
            state = ?self.state,
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::InjectMessage(nonce, InjectionMessageData::FindNode(data)) => {
                let target = data.target;

                let closest_route = context
                    .routing_table()
                    .closest(&target, 20, self.config.shared_prefix_grouping)
                    .expect("grouping has to be checked on init")
                    .first()
                    .map(|(_, contact)| {
                        let mut route = SourceRoute::from(contact.path().clone());
                        route.push_front(*context.root_id());
                        route
                    });

                if closest_route.is_none() {
                    if let Err(e) = self
                        .injection_result_sender
                        .send_result(InjectionResult::Isolated)
                    {
                        log::error!(target: "inject_messages", "failed to send inject result: {e}");
                        return Err(InjectMessageError::SendResultFailed);
                    }
                    return Err(InjectMessageError::Isolated);
                }
                let source_route = closest_route.unwrap();

                log::trace!(target: "inject_messages", "Sending FindNodeReq from {} with target {}", source_route.source(), &data.target);

                let nonce = nonce.unwrap_or_else(|| {
                    // generate distinct nonce
                    loop {
                        let nonce = Nonce::random();
                        if !self.nonces.contains_key(&nonce) {
                            break nonce;
                        }
                    }
                });

                let message = ProtocolMessage::FindNodeReq(ReqRspMessage {
                    common_header : CommonHeader::new(ProtocolMessageKind::FindNodeReq,
                                                      *context.root_id(),
                                                      target,
                                                      Some(nonce.into()),
                                                      Some(From::from(*context.uln_table().state_seq_nr()))),
                    data,
                    not_via: context.not_via().clone(),
                    source_route,
                });

                context
                    .runtime()
                    .send_message(message, context.uln_table().deref());

                self.nonces.insert(nonce, context.runtime().current_time());
            }
            UseCaseEvent::Message(message, interface) => {
                if let Some(Some(instant)) = message.msg_id().map(|nonce| self.nonces.remove(&nonce))
                {
                    let elapsed = instant.elapsed();
                    if let Err(e) = self
                        .injection_result_sender
                        .send_result(InjectionResult::Answered((message.clone(), interface)))
                    {
                        log::error!(target: "inject_messages", "Sending answered result failed: {e}");
                        return Err(InjectMessageError::SendResultFailed);
                    }
                    log::trace!(target: "inject_messages", "Received response for nonce {:?} after {:?}", message.msg_id(), elapsed);
                }
            }
            _ => {}
        }

        Ok(())
    }
}

impl<C, TS, const BUCKET_SIZE: usize> UseCase for InjectMessages<C, TS, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    TS: InjectionResultSender,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &C) -> Result<(), Self::Error> {
        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

pub mod errors {
    use derive_more::{Display, Error};
    use std::fmt::Debug;

    #[derive(Debug, Display, Error)]
    pub enum InjectMessageError {
        #[display("Sending injection result failed")]
        SendResultFailed,
        #[display("Node is isolated")]
        Isolated,
    }
}
