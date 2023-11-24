use std::collections::{HashMap, HashSet};
use std::hash::Hash;

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
    type Composite = HashMap<NodeId, HashSet<TimedValue<DefaultLHTOutput>>>;
    type OutputData = Vec<DefaultLHTOutput>;
    type Error = FetchErr;

    fn fetch(&self, handle: &Self::Handle, from: &mut Self::Composite) -> Result<Self::OutputData, Self::Error> {
        match from.get_mut(handle) {
            None => {
                Err(FetchErr::NotFoundErr)
            }
            Some(set) => {
                // todo fix this here we need to copy the Arc
                Ok(Vec::from_iter(set.map(|tv| tv.value)))
            }
        }
    }
}