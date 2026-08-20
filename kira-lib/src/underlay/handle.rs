//! Interacting with an [UnderlayObserverConnection].
//!
//! The main struct is the [UnderlayObserverHandle].

use derive_more::derive::{
    Display,
    Error,
    From,
};
use futures::{
    SinkExt,
    channel::{
        mpsc::{
            SendError,
            UnboundedSender,
        },
        oneshot,
    },
};
use kira_forwarding::underlay::{
    UnderlayInformationProvider,
    UnderlayNeighborInformation,
};

// docs
#[allow(unused_imports)]
use super::*;
use crate::{
    domain::underlay::{
        InterfaceId,
        UnderlayNeighbor,
        UnderlayNeighborId,
    },
    underlay::information_base::UnderlayNeighborInterfaceDownError,
};

/// Sender used by the [UnderlayObserverHandle]
/// to send request to the [UnderlayObserverConnection].
type UnderlayObserverHandleTx = UnboundedSender<UnderlayObserverHandleRequest>;

/// Handle to inspect an [UnderlayInformationBase].
///
/// The [UnderlayInformationBase] is managed by the [UnderlayObserverConnection].
/// This struct is created using the [observe_underlay] function.
///
/// [UnderlayInformationBase]: super::information_base::UnderlayInformationBase
#[derive(Debug, Clone)]
pub struct UnderlayObserverHandle {
    tx: UnderlayObserverHandleTx,
}

#[derive(Debug)]
/// Requests relayed by the [UnderlayObserverHandle] to the
pub(super) enum UnderlayObserverHandleRequest {
    GetInformation {
        ulnid: UnderlayNeighborId,
        response: oneshot::Sender<Option<UnderlayNeighborInformation>>,
    },
    RegisterUnderlayNeighbor {
        neighbor: UnderlayNeighbor,
        response: oneshot::Sender<Result<UnderlayNeighborId, UnderlayNeighborInterfaceDownError>>,
    },
    UnregisterUnderlayNeighbor {
        ulnid: UnderlayNeighborId,
        // no response because UnderlayNeighborUpdate is "response"
    },
    GetAvailable {
        response: oneshot::Sender<Vec<InterfaceId>>,
    },
}

#[derive(Debug, Display, Error, From)]
/// Error on [UnderlayObserverHandle] failure.
pub enum UnderlayObserverHandleError {
    /// An interface is down.
    InterfaceDown(UnderlayNeighborInterfaceDownError),
    /// The result sender was closed.
    SenderClosed(UnderlayObserverSenderClosedError),
}

#[derive(Debug, Display, Error, From)]
#[display("Channel to UnderlayObserver is closed")]
/// The result sender was closed unexpectedly.
///
/// This is usually if the [UnderlayObserverConnection] was dropped before
/// interacting with the [UnderlayObserverHandle].
///
/// See [observe_underlay] on an example on how to use the handle properly.
pub struct UnderlayObserverSenderClosedError(pub SendError);

impl UnderlayObserverHandle {
    pub(super) fn new(handle_tx: UnderlayObserverHandleTx) -> Self {
        Self { tx: handle_tx }
    }

    /// Get [UnderlayNeighborInformation] of an [UnderlayNeighborId].
    ///
    /// If no underlay neighbor is known under this id [None] is returned.
    pub async fn get_information(
        &mut self,
        ulnid: &UnderlayNeighborId,
    ) -> Result<Option<UnderlayNeighborInformation>, UnderlayObserverSenderClosedError> {
        log::trace!(target: "underlay_observer::handle", "get_information {ulnid:?}");
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UnderlayObserverHandleRequest::GetInformation {
                ulnid: *ulnid,
                response: tx,
            })
            .await?;

        Ok(rx.await.expect("sender should not get dropped"))
    }

    /// Register a new [UnderlayNeighbor].
    ///
    /// Returns [UnderlayObserverHandleError::InterfaceDown] if the neighbor's interface is currently down.
    pub async fn register_neighbor(
        &mut self,
        neighbor: UnderlayNeighbor,
    ) -> Result<UnderlayNeighborId, UnderlayObserverHandleError> {
        log::trace!(target: "underlay_observer::handle", "register_neighbor {}%{}", neighbor.ll_ipv6(), neighbor.interface_id());
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UnderlayObserverHandleRequest::RegisterUnderlayNeighbor {
                neighbor,
                response: tx,
            })
            .await
            .map_err(UnderlayObserverSenderClosedError)?;

        rx.await
            .expect("sender should not get dropped")
            .map_err(From::from)
    }

    /// Unregister a [UnderlayNeighbor].
    ///
    /// This function returns nothing on success.
    /// If you want to know *if* an [UnderlayNeighbor] existed listen to the generated
    /// [UnderlayNeighborUpdate::UnderlayNeighborDown] event on
    /// the [UnderlayNeighborUpdatesRx].
    pub async fn unregister_neighbor(
        &mut self,
        ulnid: &UnderlayNeighborId,
    ) -> Result<(), UnderlayObserverSenderClosedError> {
        log::trace!(target: "underlay_observer::handle", "unregister_neighbor {ulnid}");
        self.tx
            .send(UnderlayObserverHandleRequest::UnregisterUnderlayNeighbor { ulnid: *ulnid })
            .await?;

        Ok(())
    }

    /// Get the [InterfaceIds](InterfaceId) of all interfaces that are up.
    pub async fn get_available(
        &mut self,
    ) -> Result<Vec<InterfaceId>, UnderlayObserverSenderClosedError> {
        log::trace!(target: "underlay_observer::handle", "get_available");
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UnderlayObserverHandleRequest::GetAvailable { response: tx })
            .await?;

        Ok(rx.await.expect("sender should not get dropped"))
    }
}

#[derive(Debug, Display, Error)]
/// Error for the implementation of the [UnderlayInformationProvider] trait
/// for the [UnderlayObserverHandle].
pub enum ProvidingInfoError {
    /// The result sender was closed unexpectedly.
    UnderlayObserverSenderClosed(UnderlayObserverSenderClosedError),
}

impl UnderlayInformationProvider for UnderlayObserverHandle {
    type Error = ProvidingInfoError;
    type Information = UnderlayNeighborInformation;

    async fn get_information(
        &mut self,
        ulnid: &UnderlayNeighborId,
    ) -> Result<Option<Self::Information>, Self::Error> {
        match UnderlayObserverHandle::get_information(self, ulnid).await {
            Err(e) => Err(ProvidingInfoError::UnderlayObserverSenderClosed(e)),
            Ok(info) => Ok(info),
        }
    }
}
