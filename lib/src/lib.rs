//!
//!While the Runtime handles Runtime specific tasks (like waiting a certain amount of time) the Context handles the access to Domain Objects. The Access to Domain Objects has to be done in a central component as multiple UseCases may run in parallel and the Context/Runtime decides when to start which component. The Two are split along their Tasks:
//!
//! - Context: handles Communication of the UseCases with the Domain Objects
//! - Runtime: provides Runtime specific functionality

//! Communication between UseCases is not yet considered as this would increase complexity in some Context architectures (single threaded vs multi threaded.
//!

pub mod broadcaster;
pub mod context;
pub mod domain;
pub mod messaging;
pub mod node;
pub mod runtime;
pub mod usecases;
