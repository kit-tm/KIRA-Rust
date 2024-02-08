use std::cmp::Ordering;
use std::num::NonZeroUsize;
use std::ops::IndexMut;

use rand::Rng;

use crate::domain::{
    node_id, AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, ContactState,
    GroupingError, NodeId, ReplacementError, RoutingTable, SharedPrefix, DEFAULT_BUCKET_SIZE,
};

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
#[derive(Debug)]
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

    const fn first_bucket_on_level(index: usize) -> usize {
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
        let bit_index = (node_id::BIT_SIZE).checked_sub((prefix_len + 1) * ACC);
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

    fn split_bucket(&mut self, id: &NodeId) -> Result<usize, BucketSplitError> {
        if self.buckets.len() >= Self::max_buckets() {
            return Err(BucketSplitError::MaxBucketsReached);
        }

        let bucket_index = self.get_bucket_index(id);
        log::trace!(target: "flat_routing_table", "splitting bucket with index {}", bucket_index);
        log::trace!(target: "flat_routing_table", "before: {:?}", self.buckets);
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
        log::trace!(target: "flat_routing_table", "after: {:?}", self.buckets);
        Ok(bucket_index)
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

        let first_bucket_on_level = Self::first_bucket_on_level(index);
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

    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE> {
        &self.buckets[index]
    }

    fn bucket_by_index_mut(&'a mut self, index: usize) -> Self::BucketWriteGuard {
        self.buckets.index_mut(index)
    }

    fn get_bucket_index(&self, of: &NodeId) -> usize {
        Self::get_bucket_index_for(of, &self.root, self.num_buckets())
    }

    fn get_bucket_prefix_length(&self, bucket_index: usize) -> usize {
        ACC + ACC * (bucket_index / Self::level_width())
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

        assert_eq!(table.split_bucket(&NodeId::one()), Ok(0));

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
