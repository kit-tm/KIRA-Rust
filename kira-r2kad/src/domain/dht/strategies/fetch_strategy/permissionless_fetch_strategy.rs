use std::collections::HashMap;

use crate::domain::dht::strategies::fetch_strategy::FetchStrategy;

use crate::domain::NodeId;
use crate::messaging::dht::{DefaultLHTOutput, FetchErr};
use crate::use_cases::distributed_hash_table::HashTableData;

/// This is the default implementation of the [FetchStrategy].
///
/// This strategy always succeeds in retrieving the data if it is in the composite.
/// In particular there is no need to proof one is authorized to access the data.
#[derive(Default, Debug, Clone)]
pub struct PermissionlessFetchStrategy {}

impl FetchStrategy for PermissionlessFetchStrategy {
    // TODO: use more abstract data types
    type Handle = NodeId;
    type Composite = HashMap<NodeId, HashTableData>;
    type OutputData = DefaultLHTOutput;
    type Error = FetchErr;

    fn fetch(
        &self,
        handle: &Self::Handle,
        from: &mut Self::Composite,
    ) -> Result<Self::OutputData, Self::Error> {
        self.peek(handle, from)
    }

    fn peek(
        &self,
        handle: &Self::Handle,
        from: &Self::Composite,
    ) -> Result<Self::OutputData, Self::Error> {
        match from.get(handle) {
            None => Err(FetchErr::NotFoundErr),
            Some(set) => {
                let mut vec = Vec::with_capacity(set.len());

                for timed_value in set.iter() {
                    vec.push(timed_value.value.clone())
                }

                Ok(vec)
            }
        }
    }

    fn fetch_all(
        &self,
        from: &Self::Composite,
    ) -> Result<Vec<(Self::Handle, Self::OutputData)>, Self::Error> {
        let mut dump = Vec::with_capacity(from.len());

        for (handle, set) in from.iter() {
            let mut vec = Vec::with_capacity(set.len());

            for timed_value in set.iter() {
                vec.push(timed_value.value.clone())
            }

            dump.push((*handle, vec));
        }

        Ok(dump)
    }
}

#[cfg(test)]
mod tests {
    use super::super::FetchStrategy;
    use super::*;
    use crate::domain::dht::TimedValue;
    use crate::use_cases::distributed_hash_table::HashTableSingle;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[test]
    fn test_empty_fetch() {
        let strategy = PermissionlessFetchStrategy::default();
        let mut composite = HashMap::new();
        let handle = NodeId::ZERO;

        let result = strategy.fetch(&handle, &mut composite);

        assert!(
            result.is_err(),
            "Fetching data from empty set returned an Ok result: {result:?}"
        );

        let result = result.unwrap_err();
        assert!(
            matches!(result, FetchErr::NotFoundErr),
            "Didn't return NotFoundErr as result: {result:?}"
        );
    }

    #[test]
    fn test_single_fetch() {
        let strategy = PermissionlessFetchStrategy::default();
        let mut composite = HashMap::new();
        let existing_data: HashTableSingle = Arc::new([1, 2, 3, 4]);
        let handle = NodeId::ZERO;

        composite.insert(
            handle,
            HashSet::from([TimedValue::new(existing_data.clone())]),
        );

        let result = strategy.fetch(&handle, &mut composite);

        assert!(result.is_ok(), "Fetching data failed: {result:?}");

        let result = result.unwrap();
        assert_eq!(
            result,
            vec![existing_data.clone()],
            "Didn't return right value : {result:?}"
        );
    }
}
