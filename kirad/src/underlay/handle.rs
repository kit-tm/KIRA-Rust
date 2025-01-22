//! Interacting with an [UnderlayObserverConnection](super::UnderlayObserverConnection).
//!
//! The main struct is the [UnderlayObserverHandle].

use std::net::Ipv6Addr;

use derive_more::derive::{Display, Error, From};
use futures::channel::mpsc::{SendError, UnboundedSender};
use futures::channel::oneshot;
use futures::SinkExt;

use crate::domain::underlay::{InterfaceId, UnderlayNeighborId, UnderlayNeighborInformation};
use crate::underlay::UnderlayNeighborInterfaceDownError;

/// Sender used by the [UnderlayObserverHandle]
/// to send request to the [UnderlayObserverConnection].
type UnderlayObserverHandleTx = UnboundedSender<UnderlayObserverHandleRequest>;

/// Handle to inspect an [UnderlayInformationBase].
///
/// The [UnderlayInformationBase] is managed by the [UnderlayObserverConnection].
/// This struct is created using the [observe_underlay] function.
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
        interface_id: InterfaceId,
        ll_ipv6: Ipv6Addr,
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
    pub async fn get_information(
        &mut self,
        ulnid: &UnderlayNeighborId,
    ) -> Result<Option<UnderlayNeighborInformation>, UnderlayObserverSenderClosedError> {
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
    /// If the [Interface] identified by the supplied `interface_id` this function
    /// returns [UnderlayObserverHandleError::InterfaceDown].
    pub async fn register_neighbor(
        &mut self,
        interface_id: InterfaceId,
        ll_ipv6: Ipv6Addr,
    ) -> Result<UnderlayNeighborId, UnderlayObserverHandleError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UnderlayObserverHandleRequest::RegisterUnderlayNeighbor {
                interface_id,
                ll_ipv6,
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
    /// [UnderlayNeighborUpdate::UnderlayNeighborDown] event on the [UnderlayNeighborUpdatesRx].
    pub async fn unregister_neighbor(
        &mut self,
        ulnid: &UnderlayNeighborId,
    ) -> Result<(), UnderlayObserverSenderClosedError> {
        self.tx
            .send(UnderlayObserverHandleRequest::UnregisterUnderlayNeighbor { ulnid: *ulnid })
            .await?;

        Ok(())
    }

    /// Get the [InterfaceIds](InterfaceId) of all [Interfaces](super::Interface) that are up.
    pub async fn get_available(
        &mut self,
    ) -> Result<Vec<InterfaceId>, UnderlayObserverSenderClosedError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UnderlayObserverHandleRequest::GetAvailable { response: tx })
            .await?;

        Ok(rx.await.expect("sender should not get dropped"))
    }
}
