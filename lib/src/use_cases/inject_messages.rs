use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::time::Instant;

pub use std_extension::*;
#[cfg(feature = "tokio")]
pub use tokio_extension::*;

use crate::context::UseCaseContext;
use crate::domain::{node_id, GroupingError, NetworkInterface, RoutingTable};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{Nonce, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::use_cases::inject_messages::errors::InjectMessageError;
use crate::use_cases::{
    EventHandler, InjectionMessageData, ReactiveUseCaseState, UseCase, UseCaseEvent,
};

/// Sender for [InjectionResult]s.
///
/// Used to send results for injected messages back to the outside (main component).
pub trait InjectionResultSender {
    type Error: Error;

    fn send_result(&self, result: InjectionResult) -> Result<(), Self::Error>;
}

#[cfg(feature = "tokio")]
mod tokio_extension {
    use tokio::sync::{broadcast, mpsc};

    use crate::use_cases::inject_messages::{InjectionResult, InjectionResultSender};

    impl InjectionResultSender for mpsc::Sender<InjectionResult> {
        type Error = mpsc::error::SendError<InjectionResult>;

        fn send_result(&self, result: InjectionResult) -> Result<(), Self::Error> {
            self.blocking_send(result)
        }
    }

    impl InjectionResultSender for broadcast::Sender<InjectionResult> {
        type Error = broadcast::error::SendError<InjectionResult>;

        fn send_result(&self, result: InjectionResult) -> Result<(), Self::Error> {
            self.send(result).map(|_| ())
        }
    }
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
    Answered((ProtocolMessage, NetworkInterface)),
    /// The node is isolated and the message couldn't be injected.
    Isolated,
    /// Injecting the [ProtocolMessage] failed.
    SendFailed(ProtocolMessage),
}

/// Configuration for message injection in general.
#[derive(Debug)]
pub struct InjectMessagesConfig {
    /// Grouping to use.
    ///
    /// Will be used when a FindNodeReq is injected and the closest node needs to be found.
    pub shared_prefix_grouping: NonZeroUsize,
}

impl Default for InjectMessagesConfig {
    fn default() -> Self {
        Self {
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
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
        if config.shared_prefix_grouping.get() > node_id::BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_grouping.get(),
                id_size: node_id::BIT_SIZE,
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
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    TS: InjectionResultSender,
{
    type Context = C;
    type Error = InjectMessageError;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::InjectMessage(nonce, InjectionMessageData::FindNode(data)) => {
                let target = data.target.clone();

                let closest_route = context
                    .routing_table()
                    .closest(&target, 20, self.config.shared_prefix_grouping.get())
                    .expect("grouping has to be checked on init")
                    .first()
                    .map(|(_, contact)| {
                        let mut route = SourceRoute::from(contact.path().clone());
                        route.push_front(context.root_id().clone());
                        route
                    });

                if closest_route.is_none() {
                    if let Err(e) = self
                        .injection_result_sender
                        .send_result(InjectionResult::Isolated)
                    {
                        log::error!(target: "inject_messages", "failed to send inject result: {}", e);
                        return Err(InjectMessageError::SendResultFailed);
                    }
                    return Err(InjectMessageError::Isolated);
                }
                let source_route = closest_route.unwrap();

                log::trace!(target: "inject_messages", "Sending FindNodeReq from {} with target {}", source_route.source(), &data.target);

                let message = ProtocolMessage::FindNodeReq(ReqRspMessage {
                    nonce: nonce.clone(),
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data,
                    not_via: context.not_via().clone(),
                    source_route,
                });

                if let Err(e) = context.message_sender_mut().send_message(message) {
                    log::error!(target: "inject_messages", "Failed to send triggered message: {}", e);
                    return Err(InjectMessageError::SendFailed);
                }

                self.nonces.insert(nonce, Instant::now());
            }
            UseCaseEvent::Message(message, interface) => {
                if let Some(Some(instant)) = message.nonce().map(|nonce| self.nonces.remove(nonce))
                {
                    let elapsed = instant.elapsed();
                    if let Err(e) = self
                        .injection_result_sender
                        .send_result(InjectionResult::Answered((message.clone(), interface)))
                    {
                        log::error!(target: "inject_messages", "Sending answered result failed: {}", e);
                        return Err(InjectMessageError::SendResultFailed);
                    }
                    log::trace!(target: "inject_messages", "Received response for nonce {:?} after {:?}", message.nonce(), elapsed);
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
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    TS: InjectionResultSender,
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

pub mod errors {
    use std::error::Error;
    use std::fmt::{Debug, Display, Formatter};

    #[derive(Debug)]
    pub enum InjectMessageError {
        SendResultFailed,
        SendFailed,
        Isolated,
    }

    impl Display for InjectMessageError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::SendFailed => write!(f, "Failed to send protocol message"),
                Self::SendResultFailed => write!(f, "Sending injection result failed"),
                Self::Isolated => write!(f, "Node is isolated"),
            }
        }
    }

    impl Error for InjectMessageError {}
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::NonZeroU64;

    use tokio::sync::mpsc;

    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, InsertionStrategyResult, NetworkInterface, NodeId, PNTable, Path, RoutingTable,
        StateSeqNr, TestInsertionStrategy,
    };
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, FindNodeReqData, InMemoryMessageChannel, Nonce,
        ProtocolMessage, RTableData, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::inject_messages::{
        InjectMessages, InjectMessagesConfig, InjectionResult,
    };
    use crate::use_cases::{EventHandler, InjectionMessageData, UseCase, UseCaseEvent};

    #[tokio::test]
    async fn message_sent() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let neighbor_id = NodeId::with_msb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), NetworkInterface::new("test"));

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = InjectMessagesConfig::default();

        let (result_sender, _result_receiver) = mpsc::channel(1);

        let mut use_case =
            InjectMessages::new(config, result_sender).expect("default config should be valid");

        let start_result = use_case.start(&sync_context);
        assert!(
            start_result.is_ok(),
            "Error starting use case: {:?}",
            start_result
        );

        let event_nonce = Nonce::random();
        let event_data = FindNodeReqData {
            exact: true,
            neighborhood: NonZeroU64::new(20).unwrap(),
            target: neighbor_id.clone(),
        };
        let event = UseCaseEvent::InjectMessage(
            event_nonce.clone(),
            InjectionMessageData::FindNode(event_data.clone()),
        );
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Failed to handle event: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
        assert!(
            sent_message.is_ok(),
            "Failed to receive message from hub: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message sent");
        let (sent_message, _) = sent_message.unwrap();
        if let ProtocolMessage::FindNodeReq(req) = sent_message {
            assert_eq!(
                req.nonce, event_nonce,
                "Nonce should be the one passed in inject event"
            );
            assert_eq!(
                req.data, event_data,
                "Event Data should be the one passed in inject event"
            );
        } else {
            panic!("Unexpected response sent: {:#?}", sent_message);
        }
    }

    #[test]
    fn received_answer_gets_returned() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_msb(1);
        let neighbor_id = NodeId::with_msb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = InjectMessagesConfig::default();

        let (result_sender, mut result_receiver) = mpsc::channel(1);

        let mut use_case =
            InjectMessages::new(config, result_sender).expect("default config should be valid");

        let start_result = use_case.start(&sync_context);
        assert!(
            start_result.is_ok(),
            "Error starting use case: {:?}",
            start_result
        );

        // Inject message
        let event_nonce = Nonce::random();
        let event_data = FindNodeReqData {
            exact: true,
            neighborhood: NonZeroU64::new(20).unwrap(),
            target: neighbor_id.clone(),
        };
        let event = UseCaseEvent::InjectMessage(
            event_nonce.clone(),
            InjectionMessageData::FindNode(event_data.clone()),
        );
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Failed to handle event: {:?}",
            handle_result
        );

        // Send answer
        let response_message = ProtocolMessage::FindNodeRsp(ReqRspMessage {
            nonce: event_nonce.clone(),
            source_state_seq_nr: StateSeqNr::from(1),
            data: RTableData { contacts: vec![] },
            not_via: Default::default(),
            source_route: SourceRoute::from(Path::from([neighbor_id.clone(), root_id.clone()])),
        });
        let event = UseCaseEvent::Message(response_message.clone(), interface.clone());
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Failed to handle event: {:?}",
            handle_result
        );

        // Check if inject result channel is used
        let result = result_receiver.try_recv();
        assert!(result.is_ok(), "No result sent: {:?}", result);
        let result = result.unwrap();
        assert_eq!(
            result,
            InjectionResult::Answered((response_message, interface.clone())),
            "Unexpected InjectionResult: {:?}",
            result
        );

        // Subsequent events with same nonce don't inject a result
        let response_message = ProtocolMessage::FindNodeRsp(ReqRspMessage {
            nonce: event_nonce,
            source_state_seq_nr: StateSeqNr::from(2),
            data: RTableData { contacts: vec![] },
            not_via: Default::default(),
            source_route: SourceRoute::from(Path::from([neighbor_id, root_id])),
        });
        let event = UseCaseEvent::Message(response_message, interface);
        let handle_result = use_case.handle_event(&sync_context, event);
        assert!(
            handle_result.is_ok(),
            "Failed to handle event: {:?}",
            handle_result
        );

        // Check if inject result channel is used
        let result = result_receiver.try_recv();
        assert!(result.is_err(), "Unexpected result was sent: {:?}", result);
    }
}
