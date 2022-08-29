//!
//!While the Runtime handles Runtime specific tasks (like waiting a certain amount of time) the Context handles the access to Domain Objects. The Access to Domain Objects has to be done in a central component as multiple UseCases may run in parallel and the Context/Runtime decides when to start which component. The Two are split along their Tasks:
//!
//! - Context: handles Communication of the UseCases with the Domain Objects
//! - Runtime: provides Runtime specific functionality

//! Communication between UseCases is not yet considered as this would increase complexity in some Context architectures (single threaded vs multi threaded.
//!
//! There are UseCases which rely on information in the request already added to the
//! RoutingTable.
//!
//! - OverlayNeighborhoodDiscovery: If an DeadEnd-Error occurs the contact has to be invalidated
//!     before the handle_event is called so the next FindNodeReq can go another Path.
//!     Therefore processing the information in an Error-ProtocolMessage must occur before.

pub mod broadcaster;
pub mod context;
pub mod domain;
pub mod messaging;
pub mod runtime;
pub mod use_cases;
pub mod utils;

#[cfg(test)]
pub(crate) mod tests {
    use log::LevelFilter;

    pub fn init() {
        let _ = env_logger::builder()
            .filter_level(LevelFilter::Trace)
            .is_test(true)
            .try_init();
    }
}
