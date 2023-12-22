use std::collections::{HashMap, HashSet};

use crate::domain::dht::strategies::insert_strategy::InsertionStrategy;
use crate::domain::dht::TimedValue;
use crate::domain::NodeId;
use crate::messaging::dht::{DefaultLHTInput, StoreOK, StoreResult};
use crate::use_cases::distributed_hash_table::HashTableData;

#[derive(Default, Debug, Clone)]
pub struct PermissionlessInsertStrategy {}

impl InsertionStrategy for PermissionlessInsertStrategy

{
    type Handle = NodeId;
    type Composite = HashMap<NodeId, HashTableData>;
    type InputData = DefaultLHTInput;
    type Status = StoreResult;

    fn insert(&self, handle: Self::Handle, data: Self::InputData, into: &mut Self::Composite) -> Self::Status {
        match into.get_mut(&handle) {
            None => {
                let mut set = HashSet::new();
                set.insert(TimedValue::new(data.clone()));

                into.insert(handle, set);
                Ok(StoreOK::Created)
            }
            Some(existing_data) => {
                if existing_data.replace(TimedValue::new(data.clone())).is_some() {
                    Ok(StoreOK::Created)
                } else {
                    Ok(StoreOK::Updated)
                }
            }
        }
    }
}