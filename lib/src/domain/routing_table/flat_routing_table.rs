use std::cmp::{min, Ordering};
use std::io::Read;
use std::num::NonZeroUsize;
use std::ops::IndexMut;

use bitvec::prelude::*;

use rand::Rng;
use serde_json::from_slice;

use crate::domain::{node_id, AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, ContactState, GroupingError, NodeId, ReplacementError, RoutingTable, SharedPrefix, DEFAULT_BUCKET_SIZE, DiscoveryRangeProvider};
use crate::domain::api::{DiscoveryRange, RoutingTableLayer};

pub const DEFAULT_ACCELERATION: usize = 1;

/// A [RoutingTable] implemented as flat array of [Bucket]s.
///
/// This [RoutingTable] has a root [NodeId] which the distance is computed to.
///
/// This kind of [RoutingTable] is only working with Physical Neighbor Selection
/// and Proximity Routing.
///
/// ## Improvements
///
/// Due to the missing support for const generics in const expressions (can be enabled on nightly
/// with `feature(generic_const_exprs)`.
/// Until [this issue](https://github.com/rust-lang/rust/issues/76560) is fixed, we have to stick with a Vec
#[derive(Debug,Clone)]
pub struct FlatRoutingTable<
    const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE,
    const ACC: usize = DEFAULT_ACCELERATION,
> {
    buckets: Vec<Bucket<BUCKET_SIZE>>,
    root: NodeId,
}

impl FlatRoutingTable<DEFAULT_BUCKET_SIZE, DEFAULT_ACCELERATION> {
    /// Creates a [FlatRoutingTable] with default [Bucket] size and acceleration.
    pub fn default(root: NodeId) -> Self {
        Self {
            buckets: vec![Bucket::new()],
            root,
        }
    }
}

impl<const BUCKET_SIZE: usize, const ACC: usize> From<FlatRoutingTable<BUCKET_SIZE, ACC>> for crate::domain::api::RoutingTable {
    fn from(value: FlatRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        crate::domain::api::RoutingTable {
            layers: value.get_layers(),
            acceleration_factor: ACC,
            bucket_size: BUCKET_SIZE,
        }
    }
}

impl<const BUCKET_SIZE: usize, const ACC: usize> FlatRoutingTable<BUCKET_SIZE, ACC> {
    /// Creates a [RoutingTable] with 0 capacity.
    pub fn new(root: NodeId) -> Result<Self, GroupingError> {
        Self::with_buckets(root, vec![Bucket::new()])
    }

    /// Create a [RoutingTable] with the capacity of its maximum possible number of buckets.
    ///
    /// That is equal to the [NodeId] Size in Bits.
    pub fn with_full_capacity(root: NodeId) -> Result<Self, GroupingError> {
        let mut buckets = Vec::with_capacity(Self::max_buckets());
        buckets.push(Bucket::new());
        Self::with_buckets(root, buckets)
    }

    // As soon as 'const where restrictions' are supported
    // this can be converted to a const function.
    fn with_buckets(
        root: NodeId,
        buckets: Vec<Bucket<BUCKET_SIZE>>,
    ) -> Result<Self, GroupingError> {
        if ACC > node_id::BIT_SIZE || ACC == 0 {
            return Err(GroupingError::Invalid {
                id_size: node_id::SIZE,
                group_size: ACC,
            });
        }
        Ok(Self { buckets, root })
    }

    /// Number of [Bucket]s per level dictated by *ACC*.
    pub const fn level_width() -> usize {
        (1 << ACC) - 1
    }

    // FIXME does not work for last level
    const fn first_on_level(index: usize) -> usize {
        index - (index % Self::level_width())
    }

    /// Returns a [NonZeroUsize] version of *ACC*. Workaround for using
    /// [NonZeroUsize] in const generics.
    fn non_zero_acc() -> NonZeroUsize {
        NonZeroUsize::new(ACC).unwrap()
    }

    /// Returns the max number of buckets for a [RoutingTable] with the given
    /// *ID_SIZE* and *ACC*.
    pub const fn max_buckets() -> usize {
        (node_id::BIT_SIZE / ACC) * Self::level_width()
    }

    /// Returns the number of [Bucket]s.
    pub fn num_buckets(&self) -> usize {
        self.buckets.len()
    }

    /// Returns the number of [Contact]s in this [RoutingTable].
    pub fn num_contacts(&self) -> usize {
        self.buckets.iter().flat_map(|bucket| bucket.iter()).count()
    }

    fn get_bucket_index(&self, of: &NodeId) -> usize {
        Self::get_bucket_index_for(of, &self.root, self.num_buckets())
    }

    /// Returns the index of the [Bucket] the id should be in related
    /// to the current state of the [RoutingTable].
    fn get_bucket_index_for(of: &NodeId, for_root: &NodeId, num_buckets: usize) -> usize {
        let SharedPrefix {
            xor: delta,
            length: prefix_len,
        } = for_root
            .shared_prefix_len(of, ACC)
            .expect("GroupingError after checking");

        // bitindex is now the index of the LSB of the first non-zero digit in delta
        let bit_index = node_id::BIT_SIZE.checked_sub((prefix_len + 1) * ACC);
        let bit_index = match bit_index {
            // This is the root key
            None => return num_buckets - 1, // Always at least one bucket present
            Some(bit_index) => bit_index,
        };

        // bitindex is the LSB of the first non-zero digit, so digit must not be zero
        assert_ne!(delta.bits(bit_index, Self::non_zero_acc()), Ok(0));

        assert!(
            bit_index + ACC >= node_id::BIT_SIZE
                || delta.bits(bit_index + ACC, Self::non_zero_acc()) == Ok(0)
        );

        // on each level we have levelWidth buckets:
        // levelWidth = 2^accelerationfactor - 1
        // the prefixLen denotes depth of the level
        // levelId = prefixLen
        // bucket 0 is the farthest one from my id.
        // levelBaseIndex =  levelId * levelWidth
        // levelOffset = levelWidth - digit
        // index = levelBaseIndex + levelOffset
        // => index = (prefixLen+1) * levelWidth - digit
        let index = (prefix_len + 1) * Self::level_width()
            - delta
                .bits(bit_index, Self::non_zero_acc())
                .expect("invalid index");
        assert!(index <= Self::max_buckets());

        index.min(num_buckets - 1) // Always at least one bucket present
    }

    pub fn get_layers(&self) -> Vec<RoutingTableLayer> {
        // we have one bucket less than level width
        let level_width = 2 ^ ACC - 1;
        let number_of_levels = self.num_buckets() / level_width;
        let mut result = Vec::new();
        log::warn!("Num Buckets: {}, ACC: {}, level width: {}, num levels: {}", self.num_buckets(), ACC, level_width, number_of_levels);

        let mut last_index = 0;
        for level in 0..min(number_of_levels-1, number_of_levels) { // TODO remove this hack when number of levels < 0
            let mut layer_vec = Vec::new();
            for level_index in 0..level_width {
                log::warn!("level: {}, level_index: {}", level, level_index);
                last_index = level * level_width + level_index;
                layer_vec.push(self.buckets[last_index].clone().into())
            }
            result.push(RoutingTableLayer { buckets: layer_vec } )
        }

        let mut last_layer = Vec::new();
        for i in last_index..self.num_buckets() {
            last_layer.push(self.buckets[i].clone().into())
        }

        result.push(RoutingTableLayer { buckets: last_layer });

        result
    }

    // TODO Rename this
    pub fn get_prefix_for_bucket_in_last_level(&self, last_level: usize, position_in_level: usize, fill_with_ones: bool, only_one_bucket: bool) -> NodeId {
        let bytes = self.root.clone().bytes_vec();

        let prefix_length = (last_level - 1) * ACC;

        let mut bitvec = BitVec::<u8, Msb0>::from_vec(bytes.clone());

        let bucket_offset_slice = BitSlice::<usize, Msb0>::from_element(&position_in_level);

        let offset = bucket_offset_slice.len() - ACC;
        for i in 0..ACC {
            log::warn!("prefix_length + i {}, offset + i: {}, bit: {}, slice: {}", prefix_length + i, offset + i, bucket_offset_slice[offset+i], bucket_offset_slice);
            bitvec.set(prefix_length+i, bucket_offset_slice[offset+i]);
        }

        let begin = if only_one_bucket {
            prefix_length
        } else {
            prefix_length + ACC
        };

        for i in begin..bitvec.len() {
            bitvec.set(i, fill_with_ones);
        }

        NodeId::from(<[u8; 14]>::try_from(bitvec.into_vec()).unwrap())

    }
}

impl<const BUCKET_SIZE: usize, const ACC: usize> DiscoveryRangeProvider for FlatRoutingTable<BUCKET_SIZE, ACC> {

    #[tracing::instrument(level="warn", name = "calculating discovery range", skip(self))]
    fn get_discovery_range(&self) -> DiscoveryRange {
        // only look at last layer

        let own_index = self.get_bucket_index(self.root());

        // level width returns width of incomplete levels, so last bucket ist on its own level if
        let number_levels = if self.num_buckets() == 1 {
            1
        } else {
            self.num_buckets() / Self::level_width() - 1
        };

        let mut own_found = false;
        let mut range_bucket_ids = Vec::new();
        // first_on_level does not work for
        let first_on_level = Self::first_on_level(if own_index == self.num_buckets() - 1 && own_index != 0 { own_index - 1 } else { own_index });


        log::warn!("own index: {}, number_levels: {}, first on level: {}, num_buckets: {}, level_width: {}",
            own_index, number_levels, first_on_level, self.num_buckets(), Self::level_width());

        for i in first_on_level..self.num_buckets() {
            log::warn!("i: {}, bucket len: {}", i, self.buckets[i].len());
            if self.buckets[i].len() != self.buckets[i].max_size() || i == own_index {
                range_bucket_ids.push(i);
                if i == own_index {
                    own_found = true;
                }
            } else {
                if own_found {
                    break;
                } else {
                    range_bucket_ids.clear();
                }
            }

        }

        assert!(!range_bucket_ids.is_empty());


        log::warn!("first index: {}, last index: {}", range_bucket_ids.first().unwrap(), range_bucket_ids.last().unwrap());

        let discovery_range = DiscoveryRange {
            start: self.get_prefix_for_bucket_in_last_level(number_levels, range_bucket_ids.first().unwrap() - first_on_level, false, self.num_buckets() == 1).into(),
            end: self.get_prefix_for_bucket_in_last_level(number_levels, range_bucket_ids.last().unwrap() - first_on_level, true, self.num_buckets() == 1).into(),
        };

        log::warn!("Discovery Range: {:?}", discovery_range);
        discovery_range
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<'a, BUCKET_SIZE>
    for FlatRoutingTable<BUCKET_SIZE, ACC>
{
    type ContactWriteGuard = &'a mut Contact;
    type BucketWriteGuard = &'a mut Bucket<BUCKET_SIZE>;
    type Iter = std::vec::IntoIter<&'a Contact>;
    type IterMut = std::vec::IntoIter<&'a mut Contact>;

    fn root(&self) -> &NodeId {
        &self.root
    }

    fn len(&self) -> usize {
        self.num_contacts()
    }

    fn is_empty(&self) -> bool {
        self.buckets.len() == 1 && self.buckets[0].is_empty()
    }

    fn add(&mut self, contact: Contact) -> Result<(), AddError> {
        let bucket = self.bucket_mut(contact.id());

        match bucket.insert(contact) {
            Err(BucketInsertionError::Full) => Err(AddError::NotAdded),
            Err(BucketInsertionError::DuplicateId(id)) => Err(AddError::AlreadyExists(id)),
            Ok(_) => Ok(()),
        }
    }

    fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        let bucket = self.bucket_mut(id);
        // NOTE: Maybe restructuring the RoutingTable here
        bucket.remove(id)
    }

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError> {
        let bucket = self.bucket_mut(id);
        bucket.replace(id, with)
    }

    fn contact(&self, id: &NodeId) -> Option<&Contact> {
        let bucket = self.bucket(id);
        bucket.get(id)
    }

    fn random_id(&self) -> Option<&NodeId> {
        let mut rng = rand::thread_rng();
        let random_contact = rng.gen_range(0..self.num_contacts());
        self.iter().nth(random_contact).map(|contact| contact.id())
    }

    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard> {
        let bucket = self.bucket_mut(id);
        bucket.get_mut(id)
    }

    fn contains(&self, id: &NodeId) -> bool {
        let bucket = self.bucket(id);
        bucket.contains(id)
    }

    fn split_bucket(&mut self, id: &NodeId) -> Result<(), BucketSplitError> {
        if self.buckets.len() >= Self::max_buckets() {
            return Err(BucketSplitError::MaxBucketsReached);
        }

        let bucket_index = self.get_bucket_index(id);
        if bucket_index != self.buckets.len() - 1 {
            return Err(BucketSplitError::Unsplittable);
        }
        let bucket = self.buckets.remove(bucket_index);

        self.buckets.push(Bucket::new());
        self.buckets.push(Bucket::new());

        for contact in bucket {
            if let Err(e) = self.add(contact) {
                panic!("Error inserting after splitting last bucket: {}", e);
            }
        }

        Ok(())
    }

    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        let index = self.get_bucket_index(of);
        &self.buckets[index]
    }

    fn bucket_mut(&'a mut self, of: &NodeId) -> Self::BucketWriteGuard {
        let index = self.get_bucket_index(of);
        self.buckets.index_mut(index)
    }

    /// Collects the closest `n` contacts to the given node by iterating the buckets from
    /// the one the given node would belong to.
    fn closest(
        &self,
        to: &NodeId,
        n: usize,
        shared_prefix_grouping: usize,
    ) -> Result<Vec<(SharedPrefix, Contact)>, GroupingError> {
        let index = self.get_bucket_index(to);

        // Longer prefix has to be in front
        let sorter = |first: &(SharedPrefix, Contact), second: &(SharedPrefix, Contact)| {
            if first.0 < second.0 {
                return Ordering::Less;
            }
            if first.0 == second.0 && first.1.id() < second.1.id() {
                return Ordering::Less;
            }

            Ordering::Greater
        };

        // copy all valid elements from buckets[index] to result
        let mut result = Vec::with_capacity(n);
        for contact in &self.buckets[index] {
            if contact.state() != &ContactState::Valid {
                continue;
            }

            let prefix = to.shared_prefix_len(contact.id(), shared_prefix_grouping)?;
            result.push((prefix, contact.clone()));
        }

        result.sort_by(sorter);

        if result.len() == n || self.buckets.len() == 1 {
            // return early if enough contacts were found or only 1 bucket exists
            return Ok(result);
        }

        let first_bucket_on_level = Self::first_on_level(index);
        let level_width = Self::level_width();

        for (iteration_count, bucket) in self
            .buckets
            .iter()
            .skip(first_bucket_on_level)
            .step_by(level_width)
            .enumerate()
        {
            let mut bucket_contents = Vec::with_capacity(2 * BUCKET_SIZE);
            let level = first_bucket_on_level + iteration_count * level_width;

            // if this is the last bucket copy content
            if level == self.buckets.len() - 1 && level != index {
                for contact in bucket.iter() {
                    if contact.state() != &ContactState::Valid {
                        continue;
                    }

                    let prefix = to.shared_prefix_len(contact.id(), shared_prefix_grouping)?;
                    bucket_contents.push((prefix, contact.clone()));
                }
            } else {
                // else copy whole level
                for bucket in self.buckets.iter().skip(level).take(level_width) {
                    for contact in bucket.iter() {
                        if contact.state() != &ContactState::Valid {
                            continue;
                        }

                        let prefix = to.shared_prefix_len(contact.id(), shared_prefix_grouping)?;
                        bucket_contents.push((prefix, contact.clone()));
                    }
                }
            }

            bucket_contents.sort_by(sorter);

            let remaining_contacts = n - result.len();
            let next_contacts = bucket_contents.into_iter().take(remaining_contacts);
            result.extend(next_contacts);
        }

        // if we still do not have enough contacts, we go up the tree
        for bucket in self
            .buckets
            .iter()
            .rev()
            .skip(self.buckets.len() - first_bucket_on_level)
        {
            for contact in bucket.iter() {
                let mut bucket_contents = Vec::with_capacity(2 * BUCKET_SIZE);

                if contact.state() != &ContactState::Valid {
                    continue;
                }

                let prefix = to.shared_prefix_len(contact.id(), shared_prefix_grouping)?;
                bucket_contents.push((prefix, contact.clone()));
                bucket_contents.sort_by(sorter);

                let remaining_contacts = n - result.len();
                let next_contacts = bucket_contents.into_iter().take(remaining_contacts);
                result.extend(next_contacts);
            }
            if result.len() >= n {
                return Ok(result);
            }
        }
        Ok(result)
    }

    fn iter(&'a self) -> Self::Iter {
        self.buckets
            .iter()
            .flat_map(|bucket| bucket.into_iter())
            // FIXME: Remove allocation
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn iter_mut(&'a mut self) -> Self::IterMut {
        self.buckets
            .iter_mut()
            .flat_map(|bucket| bucket.into_iter())
            // FIXME: Remove allocation
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn should_send_neighbor_sums(&self, node_id: &NodeId) -> bool {
        //if self.num_buckets() <= ACC + 1 {
        //    return true;
        //}
        //let index = self.get_bucket_index(node_id);
        //let min = if self.num_buckets() >= ACC + 1 {
        //    self.num_buckets() - ACC - 1
        //} else {
        //    0
        //};
//
        //let result = index >= min;
        //log::warn!("Sending xor sums: {}, node_id: {}", result, node_id);
        //result
        true
    }

}

#[cfg(test)]
mod routing_tests {
    use std::error::Error;

    use crate::domain::{
        AddError, Contact, FlatRoutingTable, NodeId, Path, RoutingTable, StateSeqNr,
    };

    #[test]
    fn test_add() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        let contact = Contact::new(Path::from(NodeId::one()), StateSeqNr::from(1));

        assert_eq!(table.add(contact), Ok(()));

        Ok(())
    }

    #[test]
    fn test_add_full() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.add(Contact::new(
            Path::from(NodeId::with_lsb(1)),
            StateSeqNr::from(0),
        ))?;

        assert_eq!(
            table.add(Contact::new(
                Path::from(NodeId::with_lsb(2)),
                StateSeqNr::from(0),
            )),
            Err(AddError::NotAdded)
        );

        Ok(())
    }

    #[test]
    fn test_split() -> Result<(), Box<dyn Error>> {
        // Split should move contacts accordingly and bucket_index should change

        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.add(Contact::new(Path::from(NodeId::one()), StateSeqNr::from(0)))?;

        assert_eq!(table.split_bucket(&NodeId::one()), Ok(()));

        assert_eq!(table.get_bucket_index(&NodeId::one()), 1);

        Ok(())
    }

    #[test]
    fn test_insert_to_max_buckets() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000001)),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000010)),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000100)),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00001000)),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00010000)),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00100000)),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b01000000)),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b10000000)),
            StateSeqNr::from(0),
        ))?;

        assert!(table
            .insert(Contact::new(
                Path::from(NodeId::with_lsb(0b00010111)),
                StateSeqNr::from(0),
            ))
            .is_err());
        assert_eq!(table.num_buckets(), FlatRoutingTable::<1, 1>::max_buckets());

        Ok(())
    }
}
