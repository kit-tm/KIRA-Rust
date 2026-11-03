use std::collections::HashMap;

use crate::{
    domain::{
        NodeId,
        Nonce,
    },
    use_cases::TimerId,
};

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

        let old_timer_id = match self.nonce_to_id.get_mut(&nonce) {
            Some((_node_id, timer_id)) => {
                let old_timer_id = *timer_id;
                *timer_id = new_timer_id;
                old_timer_id
            }
            None => return Err(errors::ModificationError::NonceDoesNotExistError),
        };

        // keep the timer mapping in sync with the nonce mapping
        let (node_id, mapped_nonce) = self
            .timer_to_node
            .remove(&old_timer_id)
            .expect("nonce exists, therefore its timer has to be tracked");
        assert_eq!(mapped_nonce, nonce);
        self.timer_to_node.insert(new_timer_id, (node_id, nonce));

        Ok(Some(old_timer_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::NodeId;

    fn node(v: u128) -> NodeId {
        NodeId::from(v)
    }

    fn nonce(v: u64) -> Nonce {
        Nonce::from(v)
    }

    fn timer(v: usize) -> TimerId {
        TimerId::from(v)
    }

    #[test]
    fn insert_and_lookup() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();

        assert!(map.timer_exists(&timer(10)));
        assert!(map.nonce_exists(&nonce(100)));
        assert_eq!(
            map.get_value_for_timer(&timer(10)),
            Some(&(node(1), nonce(100)))
        );
        assert_eq!(
            map.get_value_for_nonce(nonce(100)),
            Some(&(node(1), timer(10)))
        );
        assert!(map.exists(&node(1)));
        assert!(!map.exists(&node(2)));
    }

    #[test]
    fn insert_rejects_duplicate_timer() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();

        assert!(matches!(
            map.insert((node(2), timer(10), nonce(200))),
            Err(errors::InsertionError::DuplicateTimer)
        ));
        // failed insertion must not leave partial state
        assert!(!map.nonce_exists(&nonce(200)));
    }

    #[test]
    fn insert_rejects_duplicate_nonce() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();

        assert!(matches!(
            map.insert((node(2), timer(20), nonce(100))),
            Err(errors::InsertionError::DuplicateNonce)
        ));
        assert!(!map.timer_exists(&timer(20)));
    }

    #[test]
    fn remove_by_nonce_untracks_both_directions() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();

        assert_eq!(map.remove_by_nonce(nonce(100)), Some((node(1), timer(10))));
        assert!(!map.nonce_exists(&nonce(100)));
        assert!(!map.timer_exists(&timer(10)));
        assert!(!map.exists(&node(1)));
        assert_eq!(map.remove_by_nonce(nonce(100)), None);
    }

    #[test]
    fn remove_by_timer_untracks_both_directions() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();

        assert_eq!(map.remove_by_timer(&timer(10)), Some((node(1), nonce(100))));
        assert!(!map.nonce_exists(&nonce(100)));
        assert!(!map.timer_exists(&timer(10)));
        assert_eq!(map.remove_by_timer(&timer(10)), None);
    }

    #[test]
    fn remove_by_id_only_affects_given_node() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();
        map.insert((node(1), timer(11), nonce(101))).unwrap();
        map.insert((node(2), timer(20), nonce(200))).unwrap();

        map.remove_by_id(&node(1));

        assert!(!map.timer_exists(&timer(10)));
        assert!(!map.timer_exists(&timer(11)));
        assert!(!map.nonce_exists(&nonce(100)));
        assert!(!map.nonce_exists(&nonce(101)));
        assert!(map.timer_exists(&timer(20)));
        assert!(map.nonce_exists(&nonce(200)));
        assert!(!map.exists(&node(1)));
        assert!(map.exists(&node(2)));
    }

    #[test]
    fn replace_timer_for_nonce_keeps_timer_map_in_sync() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();

        let removed = map.replace_timer_for_nonce(nonce(100), timer(11)).unwrap();

        assert_eq!(removed, Some(timer(10)));
        assert!(map.timer_exists(&timer(11)), "new timer must be tracked");
        assert!(
            !map.timer_exists(&timer(10)),
            "old timer must no longer be tracked"
        );
        assert_eq!(
            map.get_value_for_timer(&timer(11)),
            Some(&(node(1), nonce(100)))
        );
        assert_eq!(
            map.get_value_for_nonce(nonce(100)),
            Some(&(node(1), timer(11)))
        );

        // removing via the replaced timer has to untrack everything
        assert_eq!(map.remove_by_timer(&timer(11)), Some((node(1), nonce(100))));
        assert!(!map.nonce_exists(&nonce(100)));
    }

    #[test]
    fn replace_timer_for_nonce_rejects_existing_timer() {
        let mut map = InflightReqMap::default();
        map.insert((node(1), timer(10), nonce(100))).unwrap();
        map.insert((node(2), timer(20), nonce(200))).unwrap();

        assert!(matches!(
            map.replace_timer_for_nonce(nonce(100), timer(20)),
            Err(errors::ModificationError::DuplicateTimerError)
        ));
        // failed modification must not change the nonce mapping
        assert_eq!(
            map.get_value_for_nonce(nonce(100)),
            Some(&(node(1), timer(10)))
        );
    }

    #[test]
    fn replace_timer_for_nonce_rejects_unknown_nonce() {
        let mut map = InflightReqMap::default();

        assert!(matches!(
            map.replace_timer_for_nonce(nonce(100), timer(10)),
            Err(errors::ModificationError::NonceDoesNotExistError)
        ));
        assert!(!map.timer_exists(&timer(10)));
    }
}

pub mod errors {
    use derive_more::{
        Display,
        Error,
    };

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
