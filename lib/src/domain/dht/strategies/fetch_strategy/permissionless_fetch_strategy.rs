use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::domain::dht::strategies::fetch_strategy::FetchStrategy;

use crate::domain::dht::TimedValue;
use crate::domain::NodeId;
use crate::messaging::dht::{DefaultLHTOutput, FetchErr};

pub struct PermissionlessFetchStrategy {}

impl Default for PermissionlessFetchStrategy {
    fn default() -> Self {
        Self {}
    }
}

impl FetchStrategy for PermissionlessFetchStrategy
{
    type Handle = NodeId;
    type Composite = HashMap<NodeId, HashSet<TimedValue<Arc<[u8]>>>>;
    type OutputData = DefaultLHTOutput;
    type Error = FetchErr;

    fn fetch(&self, handle: &Self::Handle, from: &mut Self::Composite) -> Result<Self::OutputData, Self::Error> {
        match from.get_mut(handle) {
            None => {
                Err(FetchErr::NotFoundErr)
            }
            Some(set) => {
                let mut vec = Vec::with_capacity(set.capacity());

                for timed_value in set.iter() {
                    vec.push(timed_value.value.clone())
                }

                Ok(vec)
            }
        }
    }
}