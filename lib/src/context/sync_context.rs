use std::cell::RefCell;

use crate::context::{ReadGuard, UseCaseContext, WriteGuard};
use crate::domain::{NodeId, PNTable};

/// Implements a [UseCaseContext] which can only be used in a single threaded synchronous environment.
///
/// In comparison to implementations like [TokioContext] this implementation is free of locks and
/// their runtime overhead due to the assumption that everything happens in a single thread.
#[derive(Debug)]
pub struct SyncContext<RT, MS, RU, IS, FT> {
    root_id: NodeId,
    routing_table: RefCell<RT>,
    insertion_strategy: RefCell<IS>,
    pn_table: RefCell<PNTable>,
    message_sender: RefCell<MS>,
    runtime: RU,
    forwarding_tables: RefCell<FT>,
}

impl<RT, MS, RU, IS, FT> SyncContext<RT, MS, RU, IS, FT> {
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId,
        routing_table: RT,
        pn_table: PNTable,
        insertion_strategy: IS,
        message_sender: MS,
        runtime: RU,
        forwarding_tables: FT,
    ) -> Self {
        Self {
            root_id,
            routing_table: RefCell::new(routing_table),
            insertion_strategy: RefCell::new(insertion_strategy),
            pn_table: RefCell::new(pn_table),
            message_sender: RefCell::new(message_sender),
            runtime,
            forwarding_tables: RefCell::new(forwarding_tables),
        }
    }
}

impl<RT, MS, RU, IS, FT> UseCaseContext for SyncContext<RT, MS, RU, IS, FT> {
    type RoutingTable = RT;
    type MessageSender = MS;
    type Runtime = RU;
    type InsertionStrategy = IS;
    type ForwardingTables = FT;

    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> ReadGuard<RT> {
        self.routing_table.borrow().into()
    }

    fn routing_table_mut(&self) -> WriteGuard<RT> {
        self.routing_table.borrow_mut().into()
    }

    fn routing_table_insertion_strategy(&self) -> WriteGuard<IS> {
        self.insertion_strategy.borrow_mut().into()
    }

    fn pn_table(&self) -> ReadGuard<PNTable> {
        self.pn_table.borrow().into()
    }

    fn pn_table_mut(&self) -> WriteGuard<PNTable> {
        self.pn_table.borrow_mut().into()
    }

    fn message_sender(&self) -> ReadGuard<MS> {
        self.message_sender.borrow().into()
    }

    fn message_sender_mut(&self) -> WriteGuard<MS> {
        self.message_sender.borrow_mut().into()
    }

    fn forwarding_tables(&self) -> ReadGuard<'_, Self::ForwardingTables> {
        self.forwarding_tables.borrow().into()
    }

    fn forwarding_tables_mut(&self) -> WriteGuard<'_, Self::ForwardingTables> {
        self.forwarding_tables.borrow_mut().into()
    }

    fn runtime(&self) -> &RU {
        &self.runtime
    }
}
