use std::collections::HashMap;

use crate::domain::NodeId;
use crate::messaging::Nonce;
use crate::use_cases::TimerId;

/// Data structure to keep track of inflight request messages
///
#[derive(Debug, Default)]
pub struct InflightReqMap {
    timer_to_node: HashMap<TimerId, (NodeId, Nonce)>,
    nonce_to_id: HashMap<Nonce, (NodeId, TimerId)>,
}

impl InflightReqMap {
    /// Inserts a tracked message with initial [TimerId], [Nonce] and [NodeId] into the map.
    pub fn insert(&mut self, key: (NodeId, TimerId, Nonce)) -> Result<(), errors::InsertionError> {
        let (node_id, timer_id, nonce) = key;

        if self.timer_to_node.contains_key(&timer_id) {
            return Err(errors::InsertionError::DuplicateTimer);
        }
        if self.nonce_to_id.contains_key(&nonce) {
            return Err(errors::InsertionError::DuplicateNonce);
        }

        self.timer_to_node.insert(timer_id, (node_id, nonce));
        self.nonce_to_id.insert(nonce, (node_id, timer_id));
        Ok(())
    }

    pub fn nonce_exists(&self, nonce: &Nonce) -> bool {
        self.nonce_to_id.contains_key(nonce)
    }

    pub fn timer_exists(&self, timer_id: &TimerId) -> bool {
        self.timer_to_node.contains_key(timer_id)
    }

    /// Returns the [NodeId] associated with the given [Nonce].
    pub fn get_value_for_nonce(&self, nonce: Nonce) -> Option<&(NodeId, TimerId)> {
        self.nonce_to_id.get(&nonce)
    }

    /// Returns the [NodeId] associated with the given [TimerId].
    pub fn get_value_for_timer(&self, timer: &TimerId) -> Option<&(NodeId, Nonce)> {
        self.timer_to_node.get(timer)
    }

    /// Returns true if an entry exists for the given node_id
    /// NOTE: this may have runtime O(n) and should not be called too often
    pub fn exists(&self, node_id: &NodeId) -> bool {
        self.nonce_to_id
            .iter()
            .find(|(_, (n, _))| n == node_id)
            .is_some()
    }

    /// Removes an entry with its associated timer and nonce from tracking and
    /// uses the [NodeId] associated with the given [Nonce].
    pub fn remove_by_nonce(&mut self, nonce: Nonce) -> Option<(NodeId, TimerId)> {
        let (node_id, timer_id) = self.nonce_to_id.remove(&nonce)?;
        let (node_id_t, nonce_t) = self.timer_to_node.remove(&timer_id)?;
        assert_eq!(node_id, node_id_t);
        assert_eq!(nonce, nonce_t);
        Some((node_id, timer_id))
    }

    /// Removes an entry with its associated timer and nonce from tracking and
    /// uses the [NodeId] associated with the given [TimerId].
    pub fn remove_by_timer(&mut self, timer_id: &TimerId) -> Option<(NodeId, Nonce)> {
        let (node_id_t, nonce) = self.timer_to_node.remove(timer_id)?;
        let (node_id, timer_id_n) = self.nonce_to_id.remove(&nonce)?;
        assert_eq!(node_id, node_id_t);
        assert_eq!(timer_id, &timer_id_n);
        Some((node_id, nonce))
    }

    /// Removes entries for the given contact
    /// /// NOTE: this may have runtime O(n) and should not be called too often
    pub fn remove_by_id(&mut self, node_id: &NodeId) {
        self.timer_to_node.retain(|_, (nid, _)| nid != node_id);
        self.nonce_to_id.retain(|_, (nid, _)| nid != node_id);
    }

    /// Replaces a timer associated with the nonce
    pub fn replace_timer_for_nonce(
        &mut self,
        nonce: Nonce,
        new_timer_id: TimerId,
    ) -> Result<Option<TimerId>, errors::ModificationError> {
        if self.timer_to_node.contains_key(&new_timer_id) {
            return Err(errors::ModificationError::DuplicateTimerError);
        }
        let removed_timer: Option<TimerId>;
        // find timer and update it
        if let Some((_node_id, timer_id)) = self.nonce_to_id.get_mut(&nonce) {
            removed_timer = Some(*timer_id);
            *timer_id = new_timer_id;
        } else {
            return Err(errors::ModificationError::NonceDoesNotExistError);
        }

        Ok(removed_timer)
    }
}

pub mod errors {
    use derive_more::{Display, Error};

    #[derive(Debug, Display, Error)]
    pub enum InsertionError {
        #[display("Duplicate Timer: Only a single timer is allowed for a message")]
        DuplicateTimer,
        #[display("Duplicate Nonce: Tried to insert an already existing nonce")]
        DuplicateNonce,
    }

    #[derive(Debug, Display, Error)]
    pub enum ModificationError {
        #[display("Duplicate Timer: Timer exists already")]
        DuplicateTimerError,
        #[display("Duplicate Nonce: Nonce exists already")]
        DuplicateNonceError,
        #[display("Unknown Nonce: cannot find this nonce")]
        NonceDoesNotExistError,
    }

    #[derive(Debug, Display, Error)]
    pub enum AddNonceError {
        #[display("Unknown node: tried to add nonce to unknown node")]
        UnknownNode,
        #[display("Duplicate Nonce: Tried to insert an already existing nonce")]
        DuplicateNonce,
    }
}
