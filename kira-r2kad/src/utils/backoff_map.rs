use std::collections::HashMap;

use crate::domain::NodeId;
use crate::messaging::Nonce;
use crate::use_cases::TimerId;
use crate::utils::ExponentialBackoff;

/// Data structure to keep track of the exponential backoff of contacts.
///
/// Per node multiple nonces for messages in flight and a single TimerId can be present.
#[derive(Debug, Default)]
pub struct BackoffMap {
    node_to_state: HashMap<NodeId, ExponentialBackoff>,
    timer_to_node: HashMap<TimerId, NodeId>,
    nonce_to_id: HashMap<Nonce, NodeId>,
}

impl BackoffMap {
    /// Inserts a tracked contact with initial [TimerId], [Nonce] and [ExponentialBackoff] into the map.
    pub fn insert(
        &mut self,
        key: (NodeId, TimerId, Nonce),
        value: ExponentialBackoff,
    ) -> Result<Option<ExponentialBackoff>, errors::InsertionError> {
        let (node_id, timer_id, nonce) = key;

        if self.timer_to_node.contains_key(&timer_id) {
            return Err(errors::InsertionError::DuplicateTimer);
        }
        if self.nonce_to_id.contains_key(&nonce) {
            return Err(errors::InsertionError::DuplicateNonce);
        }

        self.timer_to_node.insert(timer_id, node_id);
        self.nonce_to_id.insert(nonce, node_id);
        Ok(self.node_to_state.insert(node_id, value))
    }

    /// Returns a mutable reference to the registered exponential backoff for the given id.
    pub fn get_mut(&mut self, id: &NodeId) -> Option<&mut ExponentialBackoff> {
        self.node_to_state.get_mut(id)
    }

    /// Returns the registered exponential backoff for the given id.
    pub fn get(&mut self, id: &NodeId) -> Option<&ExponentialBackoff> {
        self.node_to_state.get(id)
    }

    /// Returns the [NodeId] associated with the given [Nonce].
    pub fn get_node_for_nonce(&self, nonce: &Nonce) -> Option<&NodeId> {
        self.nonce_to_id
            .get(nonce)
            .filter(|id| self.node_to_state.contains_key(id))
    }

    /// Returns the [NodeId] associated with the given [TimerId].
    pub fn get_node_for_timer(&self, timer: &TimerId) -> Option<&NodeId> {
        self.timer_to_node
            .get(timer)
            .filter(|id| self.node_to_state.contains_key(id))
    }

    /// Adds an additional [Nonce] to the [NodeId].
    pub fn add_nonce_for_node(
        &mut self,
        node: NodeId,
        nonce: Nonce,
    ) -> Result<(), errors::AddNonceError> {
        if !self.node_to_state.contains_key(&node) {
            return Err(errors::AddNonceError::UnknownNode);
        }
        if self.nonce_to_id.contains_key(&nonce) {
            return Err(errors::AddNonceError::DuplicateNonce);
        }
        self.nonce_to_id.insert(nonce, node);

        Ok(())
    }

    /// Removes a tracked [Nonce] and returns if the [Nonce] was even tracked.
    pub fn remove_nonce(&mut self, nonce: &Nonce) -> bool {
        self.nonce_to_id.remove(nonce).is_some()
    }

    /// Removes an [ExponentialBackoff] with its associated timer and nonces from tracking.
    pub fn remove(&mut self, id: &NodeId) -> Option<(ExponentialBackoff, TimerId, Vec<Nonce>)> {
        self.node_to_state.remove(id).map(|state| {
            (
                state,
                self.remove_timer_for(id).unwrap(),
                self.remove_nonces_for(id),
            )
        })
    }

    /// Removes an [ExponentialBackoff] with its associated timer and nonces from tracking and
    /// uses the [NodeId] associated with the given [Nonce].
    pub fn remove_by_nonce(
        &mut self,
        nonce: &Nonce,
    ) -> Option<(ExponentialBackoff, TimerId, Vec<Nonce>)> {
        let node_id = self.nonce_to_id.remove(nonce)?;
        let removed_timers = self.remove_timer_for(&node_id)?;
        let mut removed_nonces = self.remove_nonces_for(&node_id);
        removed_nonces.push(*nonce);
        self.node_to_state
            .remove(&node_id)
            .map(|state| (state, removed_timers, removed_nonces))
    }

    /// Removes an [ExponentialBackoff] with its associated timer and nonces from tracking and
    /// uses the [NodeId] associated with the given [TimerId].
    pub fn remove_by_timer(
        &mut self,
        timer_id: &TimerId,
    ) -> Option<(ExponentialBackoff, TimerId, Vec<Nonce>)> {
        let node_id = self.timer_to_node.remove(timer_id)?;
        let removed_nonces = self.remove_nonces_for(&node_id);
        self.node_to_state
            .remove(&node_id)
            .map(|state| (state, *timer_id, removed_nonces))
    }

    /// Replaces a timer associated with the [NodeId].
    pub fn replace_timer_for(
        &mut self,
        id: NodeId,
        timer_id: TimerId,
    ) -> Result<Option<TimerId>, errors::DuplicateTimerError> {
        if self.timer_to_node.contains_key(&timer_id) {
            return Err(errors::DuplicateTimerError);
        }
        let removed = self.remove_timer_for(&id);
        self.timer_to_node.insert(timer_id, id);
        Ok(removed)
    }

    fn remove_timer_for(&mut self, id: &NodeId) -> Option<TimerId> {
        let timer = self.timer_to_node.iter().find_map(
            |(key, value)| {
                if value == id { Some(*key) } else { None }
            },
        );
        if let Some(timer_id) = &timer {
            self.timer_to_node.remove(timer_id);
        }
        timer
    }

    fn remove_nonces_for(&mut self, id: &NodeId) -> Vec<Nonce> {
        let nonces: Vec<_> = self
            .nonce_to_id
            .iter()
            .filter_map(
                |(nonce, entry_id)| {
                    if entry_id == id { Some(*nonce) } else { None }
                },
            )
            .collect();
        for nonce in &nonces {
            self.nonce_to_id.remove(nonce);
        }

        nonces
    }
}

pub mod errors {
    use derive_more::{Display, Error};

    #[derive(Debug, Display, Error)]
    pub enum InsertionError {
        #[display("Duplicate Timer: Only a single backoff timer is allowed for a node")]
        DuplicateTimer,
        #[display("Duplicate Nonce: Tried to insert an already existing nonce")]
        DuplicateNonce,
    }

    #[derive(Debug, Display, Error)]
    #[display("Duplicate Timer: Only a single backoff timer is allowed for a node")]
    pub struct DuplicateTimerError;

    #[derive(Debug, Display, Error)]
    pub enum AddNonceError {
        #[display("Unknown node: tried to add nonce to unknown node")]
        UnknownNode,
        #[display("Duplicate Nonce: Tried to insert an already existing nonce")]
        DuplicateNonce,
    }
}
