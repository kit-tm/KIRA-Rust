use std::collections::{HashMap, HashSet};

use crate::domain::NodeId;
use crate::domain::dht::TimedValue;
use crate::domain::dht::strategies::insert_strategy::InsertionStrategy;
use crate::messaging::dht::{DefaultLHTInput, StoreOK, StoreResult};
use crate::use_cases::distributed_hash_table::HashTableData;

#[derive(Default, Debug, Clone)]
pub struct PermissionlessInsertStrategy {}

impl InsertionStrategy for PermissionlessInsertStrategy {
    type Handle = NodeId;
    type Composite = HashMap<NodeId, HashTableData>;
    type InputData = DefaultLHTInput;
    type Status = StoreResult;

    fn insert(
        &self,
        handle: Self::Handle,
        data: Self::InputData,
        into: &mut Self::Composite,
    ) -> Self::Status {
        match into.get_mut(&handle) {
            None => {
                let mut set = HashSet::new();
                set.insert(TimedValue::new(data.clone()));

                into.insert(handle, set);
                Ok(StoreOK::Created)
            }
            Some(existing_data) => {
                if existing_data
                    .replace(TimedValue::new(data.clone()))
                    .is_some()
                {
                    Ok(StoreOK::Updated)
                } else {
                    Ok(StoreOK::Inserted)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::InsertionStrategy;
    use super::*;

    use std::sync::Arc;

    #[test]
    fn creation() {
        let strategy = PermissionlessInsertStrategy::default();
        let mut composite = HashMap::new();
        let handle = NodeId::zero();
        let data: DefaultLHTInput = Arc::new([1, 2, 3, 4]);

        let result = strategy.insert(handle, data, &mut composite);

        assert!(result.is_ok(), "Insertion of new data failed: {result:?}");

        let result = result.unwrap();
        assert!(
            matches!(result, StoreOK::Created),
            "Didn't return Created as result: {result:?}"
        );
    }

    #[test]
    fn insertion() {
        let strategy = PermissionlessInsertStrategy::default();
        let handle = NodeId::zero();
        let mut composite = HashMap::new();
        let existing_data: DefaultLHTInput = Arc::new([4, 3, 2, 1]);
        let data: DefaultLHTInput = Arc::new([1, 2, 3, 4]);

        composite.insert(handle, HashSet::from([TimedValue::new(existing_data)]));

        let result = strategy.insert(handle, data, &mut composite);

        assert!(result.is_ok(), "Insertion of new data failed: {result:?}");

        let result = result.unwrap();
        assert!(
            matches!(result, StoreOK::Inserted),
            "Didn't return Inserted as result: {result:?}"
        );
    }

    #[test]
    fn update() {
        let strategy = PermissionlessInsertStrategy::default();
        let handle = NodeId::zero();
        let mut composite = HashMap::new();
        let existing_data: DefaultLHTInput = Arc::new([1, 2, 3, 4]);
        let data: DefaultLHTInput = Arc::new([1, 2, 3, 4]);

        composite.insert(handle, HashSet::from([TimedValue::new(existing_data)]));

        let result = strategy.insert(handle, data, &mut composite);

        assert!(result.is_ok(), "Insertion of new data failed: {result:?}");

        let result = result.unwrap();
        assert!(
            matches!(result, StoreOK::Updated),
            "Didn't return Updated as result: {result:?}"
        );
    }
}
