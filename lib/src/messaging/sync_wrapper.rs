use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Runtime;

use crate::domain::Port;
use crate::messaging::error::SenderError;
use crate::messaging::{
    AsyncProtocolMessageReceiver, AsyncProtocolMessageSender, ProtocolMessage,
    ProtocolMessageReceiver, ProtocolMessageSender, RecvError, TryRecvError,
};

/// Wrapper for [AsyncProtocolMessageReceiver] and [AsyncProtocolMessageSender] to be used
/// in a sync context.
#[derive(Debug)]
pub struct SyncWrapper<C> {
    inner: C,
    runtime: Arc<Runtime>,
}

impl<C> SyncWrapper<C> {
    pub fn new(inner: C, runtime: Arc<Runtime>) -> Self {
        Self { inner, runtime }
    }
}

impl<C> ProtocolMessageSender for SyncWrapper<C>
where
    C: AsyncProtocolMessageSender,
{
    fn send<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage>,
    {
        let message = message.into();
        self.runtime.block_on(self.inner.send(message))
    }
}

impl<C> ProtocolMessageReceiver for SyncWrapper<C>
where
    C: AsyncProtocolMessageReceiver,
{
    fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, Port)>, RecvError> {
        self.runtime.block_on(self.inner.recv_timeout(timeout))
    }

    fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, Port)>, TryRecvError> {
        self.runtime.block_on(self.inner.try_recv())
    }
}
