use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::domain::dht::strategies::insert_strategy::InsertionStrategy;
use crate::domain::dht::TimedValue;
use crate::domain::NodeId;
use crate::messaging::dht::{DefaultLHTInput, StoreOK, StoreResult};

pub struct PermissionlessInsertStrategy {}

impl Default for PermissionlessInsertStrategy {
    fn default() -> Self {
        Self {}
    }
}

impl InsertionStrategy for PermissionlessInsertStrategy

{
    type Handle = NodeId;
    type Composite = HashMap<NodeId, HashSet<TimedValue<Arc<[u8]>>>>;
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
                if let Some(_) = existing_data.replace(TimedValue::new(data.clone())) {
                    Ok(StoreOK::Created)
                } else {
                    Ok(StoreOK::Updated)
                }
            }
        }
    }
}