use std::cmp::Ordering;
use std::num::NonZeroUsize;
use std::ops::IndexMut;

use rand::Rng;

use crate::domain::{
    AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, ContactState,
    DEFAULT_BUCKET_SIZE, GroupingError, NodeId, ReplacementError, RoutingTable, SharedPrefix,
    node_id,
};

pub const DEFAULT_ACCELERATION: usize = 1;

/// A [RoutingTable] implemented as flat array of [Bucket]s.
///
/// This [RoutingTable] has a root [NodeId] which the distance is computed to.
///
/// This kind of [RoutingTable] is only working with Underlay Neighbor Selection
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
        NonZeroUsize::new(ACC).expect("checked on initialization")
    }

    /// Returns the max number of buckets for a [RoutingTable] with the given
    /// `ID_SIZE` and `ACC`.
    pub const fn max_buckets() -> usize {
        (node_id::BIT_SIZE / ACC) * Self::level_width()
    }

    /// Returns the number of present [Bucket]s.
    pub fn num_buckets(&self) -> usize {
        self.buckets.len()
    }

    /// Returns the number of [Contact]s in this [RoutingTable].
    pub fn num_contacts(&self) -> usize {
        self.buckets.iter().flat_map(|bucket| bucket.iter()).count()
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<'a, BUCKET_SIZE>
    for FlatRoutingTable<BUCKET_SIZE, ACC>
{
    type ContactWriteGuard = &'a mut Contact;
    type BucketWriteGuard = &'a mut Bucket<BUCKET_SIZE>;
    type BucketIter = std::slice::Iter<'a, Bucket<BUCKET_SIZE>>;

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
        log::trace!(target: "flat_routing_table", "splitting bucket with index {bucket_index}");
        log::trace!(target: "flat_routing_table", "before: {:?}", self.buckets);
        if bucket_index != self.buckets.len() - 1 {
            return Err(BucketSplitError::Unsplittable);
        }
        let bucket = self.buckets.remove(bucket_index);

        // create new bucket level
        for _ in 0..Self::level_width() {
            self.buckets.push(Bucket::new());
        }
        // recreate deepest bucket
        self.buckets.push(Bucket::new());

        for contact in bucket {
            if let Err(e) = self.add(contact) {
                panic!("Error inserting after splitting last bucket: {e}");
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
        // check for valid grouping first because we don't want unexpectetly panic inside iters
        to.shared_prefix_len(&self.root, shared_prefix_grouping)?;

        // sort by longest prefix
        let sorter = |first: &(SharedPrefix, Contact), second: &(SharedPrefix, Contact)| {
            if first.0 < second.0 {
                return Ordering::Less;
            }
            if first.0 == second.0 && first.1.id() < second.1.id() {
                return Ordering::Less;
            }

            Ordering::Greater
        };

        let index = self.get_bucket_index(to);

        let mut result = Vec::with_capacity(n);
        // copy all valid elements from buckets[index] to result
        let bucket_content = self.buckets[index]
            .iter()
            .filter(|c| c.state() == &ContactState::Valid)
            .map(|c| {
                let prefix = to
                    .shared_prefix_len(c.id(), shared_prefix_grouping)
                    .expect("checked shared_prefix_grouping on root");
                (prefix, c.clone())
            });
        result.extend(bucket_content);

        if result.len() >= n || self.buckets.len() == 1 {
            // return early if enough contacts were found or only 1 bucket exists
            result.sort_by(sorter);
            return Ok(result);
        }

        let first_bucket_on_level = Self::first_bucket_on_level(index);
        let level_width = Self::level_width();

        // go down the tree to find more nodes
        for (iteration_count, bucket) in self
            .buckets
            .iter()
            .skip(first_bucket_on_level)
            .step_by(level_width)
            .enumerate()
        {
            let level = first_bucket_on_level + iteration_count * level_width;

            // if this is the last bucket copy content
            if level == self.buckets.len() - 1 && level != index {
                let bucket_content = bucket
                    .iter()
                    .filter(|c| c.state() == &ContactState::Valid)
                    .map(|c| {
                        let prefix = to
                            .shared_prefix_len(c.id(), shared_prefix_grouping)
                            .expect("checked shared_prefix_grouping on root");
                        (prefix, c.clone())
                    });
                result.extend(bucket_content);
            } else {
                // else copy whole level
                let buckets_content =
                    self.buckets[level..level + level_width]
                        .iter()
                        .flat_map(|b| {
                            b.iter()
                                .filter(|c| c.state() == &ContactState::Valid)
                                .map(|c| {
                                    let prefix = to
                                        .shared_prefix_len(c.id(), shared_prefix_grouping)
                                        .expect("checked shared_prefix_grouping on root");
                                    (prefix, c.clone())
                                })
                        });
                result.extend(buckets_content);
            };

            // collected the requested amount
            if result.len() >= n {
                result.sort_by(sorter);
                return Ok(result);
            }
        }

        // if we still do not have enough contacts, we go up the tree
        for bucket in self
            .buckets
            .iter()
            .rev()
            .skip(self.buckets.len() - first_bucket_on_level)
        {
            let bucket_contents = bucket
                .iter()
                .filter(|c| c.state() == &ContactState::Valid)
                .map(|c| {
                    let prefix = to
                        .shared_prefix_len(c.id(), shared_prefix_grouping)
                        .expect("valid shared_prefix_grouping");
                    (prefix, c.clone())
                });
            result.extend(bucket_contents);
            if result.len() >= n {
                break;
            }
        }

        result.sort_by(sorter);
        Ok(result)
    }

    fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.buckets.iter().flat_map(|bucket| bucket.into_iter())
    }

    fn iter_mut(&'a mut self) -> impl Iterator<Item = Self::ContactWriteGuard> {
        self.buckets
            .iter_mut()
            .flat_map(|bucket| bucket.into_iter())
    }

    fn bucket_iter(&'a self) -> Self::BucketIter {
        self.buckets.iter()
    }

    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE> {
        &self.buckets[index]
    }

    fn bucket_by_index_mut(&'a mut self, index: usize) -> Self::BucketWriteGuard {
        self.buckets.index_mut(index)
    }

    /// Returns the index of the [Bucket] the id should be in related
    /// to the current state of the [RoutingTable].
    fn get_bucket_index(&self, of: &NodeId) -> usize {
        let SharedPrefix {
            xor: delta,
            length: prefix_len,
        } = self
            .root
            .shared_prefix_len(of, ACC)
            .expect("grouping is checked on initialization");

        // bitindex is now the index of the LSB of the first non-zero digit in delta
        // Example: delta = 00 00 00 01 10 11 10; ACC=2
        // => prefix_len = 3, bit_index = 14 - 8 = 6
        let bit_index = (node_id::BIT_SIZE).checked_sub((prefix_len + 1) * ACC);
        let bit_index = match bit_index {
            // This is the root key
            None => return self.num_buckets() - 1, // Always at least one bucket present
            Some(bit_index) => bit_index,
        };

        // Example: digit = 01
        let digit = delta.bits(bit_index, Self::non_zero_acc()).unwrap();
        assert_ne!(
            digit, 0,
            "bit_index is the LSB of the first non-zero digit, so digit must not be zero"
        );

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
        let level_id = prefix_len;
        let level_base_index = level_id * Self::level_width();
        let level_offset = Self::level_width() - digit;
        let index = level_base_index + level_offset;
        assert!(index <= Self::max_buckets());

        index.min(self.num_buckets() - 1) // Always at least one bucket present
    }

    fn get_bucket_prefix_length(&self, bucket_index: usize) -> usize {
        ACC + ACC * (bucket_index / Self::level_width())
    }
}

#[cfg(test)]
mod routing_tests {
    use std::error::Error;

    use crate::domain::{
        AddError, Contact, FlatRoutingTable, NodeId, Path, RoutingTable, SafeStateSeqNr,
    };

    #[test]
    fn test_add() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        let contact = Contact::new(
            Path::from(NodeId::one()),
            SafeStateSeqNr::try_from(2).unwrap(),
        );

        assert_eq!(table.add(contact), Ok(()));

        Ok(())
    }

    #[test]
    fn test_add_full() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.add(Contact::new(
            Path::from(NodeId::with_lsb(1)),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        assert_eq!(
            table.add(Contact::new(
                Path::from(NodeId::with_lsb(2)),
                SafeStateSeqNr::try_from(1).unwrap(),
            )),
            Err(AddError::NotAdded)
        );

        Ok(())
    }

    #[test]
    fn test_split() -> Result<(), Box<dyn Error>> {
        // Split should move contacts accordingly and bucket_index should change

        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.add(Contact::new(
            Path::from(NodeId::one()),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        assert_eq!(table.split_bucket(&NodeId::one()), Ok(0));

        assert_eq!(table.get_bucket_index(&NodeId::one()), 1);

        Ok(())
    }

    #[test]
    fn test_insert_to_max_buckets() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000001)),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000010)),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;
        assert_eq!(
            table.num_buckets(),
            FlatRoutingTable::<1, 1>::max_buckets(),
            "max buckets"
        );

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00010000)),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        assert!(
            table
                .insert(Contact::new(
                    Path::from(NodeId::with_lsb(0b00010111)),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .is_err(),
            "is in the same bucket as 00010000"
        );

        Ok(())
    }

    #[test]
    fn test_split_acc() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<2, 2>::new(NodeId::zero())?;

        table.add(Contact::new(
            Path::from(NodeId::with_lsb(0b00000001)),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;
        assert_eq!(table.buckets.len(), 1);

        assert!(
            table
                .add(Contact::new(
                    Path::from(NodeId::with_msb(0b11000000)),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .is_ok(),
            "no split required on bucket BUCKET_SIZE=2"
        );
        assert_eq!(table.buckets.len(), 1, "really, no split happend");

        // try to add node into full bucket should not work
        assert_eq!(
            table.add(Contact::new(
                Path::from(NodeId::with_msb(0b10000000)),
                SafeStateSeqNr::try_from(1).unwrap(),
            )),
            Err(AddError::NotAdded),
            "split required because single bucket is full"
        );

        // split bucket and then add node
        assert!(
            table.split_bucket(&NodeId::with_msb(0b10000000)).is_ok(),
            "split should be possible because ID would reside in lowest bucket"
        );
        assert!(
            table
                .add(Contact::new(
                    Path::from(NodeId::with_msb(0b10000000)),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .is_ok(),
            "bucket should have been created on split"
        );

        // add another node into same bucket
        assert!(
            table
                .add(Contact::new(
                    Path::from(NodeId::with_msb(0b10000001)),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .is_ok(),
            "bucket with prefix 10 should have space"
        );
        assert!(
            table
                .add(Contact::new(
                    Path::from(NodeId::with_msb(0b00000011)),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .is_ok(),
            "bucket with prefix 00 should have space"
        );

        assert!(
            table
                .add(Contact::new(
                    Path::from(NodeId::with_msb(0b01000000)),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .is_ok(),
            "bucket with prefix 01 should have space"
        );
        println!("{table:#?}");

        assert_eq!(
            table.buckets.len(),
            4,
            "split because lowest bucket is over-full"
        );

        Ok(())
    }
}
