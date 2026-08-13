use std::{
    num::NonZeroU8,
    ops::IndexMut,
};

use rand::RngExt as _;
use tracing::{
    Level,
    field,
};

use crate::domain::{
    AddError,
    Bucket,
    BucketInsertionError,
    BucketSplitError,
    Contact,
    DEFAULT_BUCKET_SIZE,
    GroupingError,
    NodeId,
    ReplacementError,
    RoutingTable,
    SharedPrefix,
    hasher::Hasher,
    routing_table::{
        PrefixContact,
        sorter_xor,
    },
};

pub const DEFAULT_ACCELERATION: u8 = 1;

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
    const ACC: u8 = DEFAULT_ACCELERATION,
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

impl<const BUCKET_SIZE: usize, const ACC: u8> FlatRoutingTable<BUCKET_SIZE, ACC> {
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
        if ACC > NodeId::BITS || ACC == 0 {
            return Err(GroupingError::Invalid {
                group_size: NonZeroU8::try_from(ACC).unwrap_or(NonZeroU8::MAX),
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
    const fn non_zero_acc() -> NonZeroU8 {
        NonZeroU8::new(ACC).expect("checked on initialization")
    }

    /// Returns the max number of buckets for a [RoutingTable] with the given
    /// `ID_SIZE` and `ACC`.
    pub const fn max_buckets() -> usize {
        ((NodeId::BITS / ACC) as usize) * Self::level_width()
    }

    /// Returns the number of present [Bucket]s.
    pub fn num_buckets(&self) -> usize {
        self.buckets.len()
    }

    /// Returns the number of [Contact]s in this [RoutingTable].
    pub fn num_contacts(&self) -> usize {
        self.buckets.iter().flat_map(|bucket| bucket.iter()).count()
    }

    fn bucket_mut(&mut self, of: &NodeId) -> &mut Bucket<BUCKET_SIZE> {
        let index = self.get_bucket_index(of);
        self.buckets.index_mut(index)
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: u8> RoutingTable<'a, BUCKET_SIZE>
    for FlatRoutingTable<BUCKET_SIZE, ACC>
{
    type BucketIter = std::slice::Iter<'a, Bucket<BUCKET_SIZE>>;
    type ContactWriteGuard = &'a mut Contact;

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
        let mut rng = rand::rng();
        let random_contact = rng.random_range(0..self.num_contacts());
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

    fn contains_with<F>(&self, id: &NodeId, f: F) -> bool
    where
        F: Fn(&Contact) -> bool,
    {
        let bucket = self.bucket(id);
        if let Some(contact) = bucket.get(id) {
            f(contact)
        } else {
            false
        }
    }

    fn is_close_contact(&self, id: &NodeId) -> bool {
        let bucket_index = self.get_bucket_index(id);
        (bucket_index + 1) == self.buckets.len() || (bucket_index + 2) == self.buckets.len()
    }

    #[tracing::instrument(
        level = Level::TRACE,
        target = "routing_table::flat_routing_table",
        skip(self),
        fields(
            before = ?self.buckets,
            after = field::Empty
        )
    )]
    fn split_bucket(&mut self, id: &NodeId) -> Result<usize, BucketSplitError> {
        if self.buckets.len() >= Self::max_buckets() {
            return Err(BucketSplitError::MaxBucketsReached);
        }

        let bucket_index = self.get_bucket_index(id);
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
                unreachable!("Error inserting after splitting last bucket: {e}");
            }
        }

        tracing::Span::current().record("after", "{self.buckets:?}");
        Ok(bucket_index)
    }

    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        let index = self.get_bucket_index(of);
        &self.buckets[index]
    }

    #[tracing::instrument(
        level = Level::TRACE,
        target = "routing_table::flat_routing_table",
        skip(self),
        ret, err
    )]
    fn closest(
        &self,
        target: &NodeId,
        n: usize,
        shared_prefix_grouping: NonZeroU8,
    ) -> Result<Vec<PrefixContact>, GroupingError> {
        // check for valid grouping first because we don't want unexpectedly panic inside iters
        target.shared_prefix_len(&self.root, shared_prefix_grouping)?;

        let mut result = Vec::with_capacity(n); // reallocation result >= n unavoidable

        let target_bucket_index = self.get_bucket_index(target);
        let last_bucket_index = self.buckets.len().saturating_sub(1);
        let level_width = Self::level_width();

        let target_level_start_index = Self::first_bucket_on_level(target_bucket_index);
        let target_level = target_level_start_index / level_width;
        assert_eq!(
            target_level_start_index % level_width,
            0,
            "The start index of a level must be a multiple of the level width ({level_width})"
        );

        // Collect contacts of last bucket.
        if target_bucket_index == last_bucket_index {
            result.extend(
                self.buckets[target_bucket_index]
                    .iter_valid_with_prefix(target, shared_prefix_grouping),
            );
            tracing::trace!(
                target: "routing_table::flat_routing_table",
                index = target_bucket_index,
                reason = "Last bucket is bucket of target",
                "Consider valid contacts of bucket"
            );

            result.sort_unstable_by(sorter_xor);

            // Prohibit return of additional valid contacts of bucket
            // to prevent incorrect proximity neighbor selection on the false
            // assumption that all provide the "same prefix progress".
            tracing::trace!(
                target: "routing_table::flat_routing_table",
                "Truncate contacts of last bucket",
            );
            result.truncate(n);

            if result.len() >= n || self.buckets.len() == 1 {
                return Ok(result);
            }
        } else {
            // Collect buckets _at_ level of target's bucket in descending xor-metric.
            //
            // We also reason about the splittable prefix to determine when to
            // collect contacts down the tree.

            // ONE: Determine the order of the buckets at level of target's bucket.

            // Example: root = 10 11 [11] 00 11 11 11
            // target_level = 2; ACC=2
            // bit_index = 14 - (2+1)*2 = 8
            // root_bits = [11]
            let bit_index = NodeId::BITS
                .checked_sub((target_level + 1) as u8 * ACC)
                .expect("target_level exceeds the maximum possible depth for the NodeId size");
            // Bits at the current level that determine the bucket index.
            let root_bits_at_level =
                self.root
                    .bits(bit_index, Self::non_zero_acc())
                    .expect("target_level depth exceeds NodeId bit size") as usize;
            let target_bits_at_level = target
                .bits(bit_index, Self::non_zero_acc())
                .expect("target_level depth exceeds NodeId bit size")
                as usize;

            // Determine the order of buckets at this level based on their XOR distance to the target.
            // We include 'level_width' to represent the path to deeper levels (the "splittable" prefix).
            let target_xor_root_at_level = root_bits_at_level ^ target_bits_at_level;
            let mut sorted_offsets_at_level: Vec<_> = (0..=level_width)
                .map(|relative_offset| {
                    // Buckets are stored in the table per level by descending xor-metric to root
                    // An additional xor by 'target_bits_at_level' yields XOR-distance to the target.
                    let bucket_xor_target =
                        (level_width - relative_offset) ^ target_xor_root_at_level;

                    (relative_offset, bucket_xor_target)
                })
                .collect();
            // sort by xor of bucket prefix bits to corresponding target bits
            sorted_offsets_at_level.sort_unstable_by_key(|(_, xor)| *xor);

            debug_assert_eq!(
                target_level * level_width + sorted_offsets_at_level[0].0,
                target_bucket_index,
                "Collect target's bucket first"
            );

            // TWO: Collect contacts of buckets in descending xor-metric order.

            let level_span = tracing::trace_span!(
                target: "routing_table::flat_routing_table",
                "Collecting nodes of level",
                level = target_level,
                reason = "Bucket-level of target",
            );
            for (bucket_offset, _) in sorted_offsets_at_level {
                if bucket_offset == level_width {
                    // Splittable bucket prefix: Explore deeper levels.
                    let deeper_buckets = &self.buckets[target_level_start_index + level_width..];
                    for (relative_depth, buckets) in deeper_buckets.chunks(level_width).enumerate()
                    {
                        let current_level = target_level + relative_depth + 1;
                        let _span = tracing::trace_span!(
                            target: "routing_table::flat_routing_table",
                            "Collecting nodes of level",
                            level = current_level,
                            dlvl = relative_depth + 1,
                        )
                        .entered();

                        let level_content =
                            buckets.iter().enumerate().flat_map(|(offset, bucket)| {
                                let index = current_level * level_width + offset;
                                assert_ne!(
                                    index, target_bucket_index,
                                    "No duplicate collection of target's bucket"
                                );

                                tracing::trace!(
                                    target: "routing_table::flat_routing_table",
                                    index,
                                    ?bucket,
                                    "Consider valid contacts of bucket"
                                );

                                bucket.iter_valid_with_prefix(target, shared_prefix_grouping)
                            });
                        result.extend(level_content);

                        // last bucket is part of last level (ACC > 1)
                        let is_at_last_level = current_level * level_width + 1 == last_bucket_index;
                        if is_at_last_level && last_bucket_index != target_bucket_index && ACC > 1 {
                            tracing::trace!(
                                target: "routing_table::flat_routing_table",
                                index = last_bucket_index,
                                reason = "Collect very last bucket as part of level",
                                "Consider valid contacts of bucket"
                            );

                            result.extend(
                                self.buckets[last_bucket_index]
                                    .iter_valid_with_prefix(target, shared_prefix_grouping),
                            );
                        }

                        if result.len() >= n {
                            result.sort_unstable_by(sorter_xor);
                            return Ok(result);
                        }
                    }
                } else {
                    // Unsplittable bucket prefix:
                    // Simply collect contacts of corresponding bucket
                    let index = target_level_start_index + bucket_offset;
                    let bucket = &self.buckets[index];

                    let _span = level_span.enter();
                    tracing::trace!(
                        target: "routing_table::flat_routing_table",
                        index,
                        ?bucket,
                        "Consider valid contacts of bucket"
                    );

                    result.extend(bucket.iter_valid_with_prefix(target, shared_prefix_grouping));

                    if result.len() >= n {
                        result.sort_unstable_by(sorter_xor);
                        return Ok(result);
                    }
                }
            }
        }

        // Collect bucket levels going up from the level of target's bucket.
        for (relative_level, buckets) in self.buckets[..target_level_start_index]
            .rchunks_exact(level_width)
            .enumerate()
        {
            let current_level = target_level - relative_level - 1;
            tracing::trace!(
                target: "routing_table::flat_routing_table",
                level = current_level,
                dlvl = -(1 + relative_level as isize),
                "Collecting nodes of level"
            );

            let level_content = buckets.iter().enumerate().flat_map(|(offset, bucket)| {
                let index = current_level * level_width + offset;
                assert_ne!(
                    index, target_bucket_index,
                    "No duplicate collection of target's bucket"
                );

                tracing::trace!(
                    target: "routing_table::flat_routing_table",
                    index,
                    ?bucket,
                    "Consider valid contacts of bucket"
                );
                bucket.iter_valid_with_prefix(target, shared_prefix_grouping)
            });
            result.extend(level_content);

            if result.len() >= n {
                result.sort_unstable_by(sorter_xor);
                return Ok(result);
            }
        }

        result.sort_unstable_by(sorter_xor);
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

    /// Returns the index of the [Bucket] the id should be in related
    /// to the current state of the [RoutingTable].
    fn get_bucket_index(&self, of: &NodeId) -> usize {
        let SharedPrefix {
            xor: delta,
            length: prefix_len,
        } = self
            .root
            .shared_prefix_len(of, Self::non_zero_acc())
            .expect("grouping is checked on initialization");

        // bitindex is now the index of the LSB of the first non-zero digit in delta
        // Example: delta = 00 00 00 01 10 11 10; ACC = 2
        // => prefix_len = 3, bit_index = 14 - (3+1)*2 = 6
        let bit_index = NodeId::BITS.checked_sub((prefix_len + 1) * ACC);
        let Some(bit_index) = bit_index else {
            // This is the root key
            return self.num_buckets() - 1; // Always at least one bucket present
        };

        debug_assert!(
            bit_index + Self::non_zero_acc().get() <= NodeId::BITS,
            "bit_index {}, acc: {}",
            bit_index,
            Self::non_zero_acc().get()
        );
        // Example: digit = 01
        let digit = delta.bits(bit_index, Self::non_zero_acc()).unwrap() as usize;
        debug_assert_ne!(
            digit, 0,
            "bit_index is the LSB of the first non-zero digit, so digit must not be zero"
        );

        debug_assert!(
            bit_index + ACC >= NodeId::BITS
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
        let level_id = prefix_len as usize;
        let level_base_index = level_id * Self::level_width();
        let level_offset = Self::level_width() - digit;
        let index = level_base_index + level_offset;
        assert!(index <= Self::max_buckets());

        index.min(self.num_buckets() - 1) // Always at least one bucket present
    }

    fn get_bucket_prefix_length(&self, bucket_index: usize) -> u8 {
        ACC + ACC * ((bucket_index / Self::level_width()) as u8)
    }

    fn path_hasher(&self) -> Hasher {
        Hasher::default()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cmp::Ordering,
        collections::HashSet,
        error::Error,
    };

    use super::*;
    use crate::domain::{
        ContactState,
        NotViaStateList,
        Path,
        SafeStateSeqNr,
    };

    fn sorted_xor<C>((a, _): &(SharedPrefix, C), (b, _): &(SharedPrefix, C)) -> bool {
        assert_ne!(a, b, "duplicate contacts");
        a.cmp(b) != Ordering::Greater
    }

    #[test]
    fn test_add() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::ZERO)?;

        let contact = Contact::new(
            Path::from(NodeId::ONE),
            SafeStateSeqNr::try_from(2).unwrap(),
        );

        assert_eq!(table.add(contact), Ok(()));

        Ok(())
    }

    #[test]
    fn test_add_full() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::ZERO)?;

        table.add(Contact::new(
            Path::from(NodeId::with_lsb(1)),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        // Adding a node to a full bucket fails
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
        crate::tests::init();
        // Split should move contacts accordingly and bucket_index should change

        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::ZERO)?;

        table.add(Contact::new(
            Path::from(NodeId::ONE),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        assert_eq!(table.split_bucket(&NodeId::ONE), Ok(0));

        assert_eq!(table.get_bucket_index(&NodeId::ONE), 1);

        Ok(())
    }

    #[test]
    fn test_insert_to_max_buckets() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::ZERO)?;

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
        crate::tests::init();
        let mut table = FlatRoutingTable::<2, 2>::new(NodeId::ZERO)?;

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
        assert_eq!(table.buckets.len(), 1, "really, no split happened");

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

    #[test]
    fn test_closest_empty() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let table = FlatRoutingTable::<2, 2>::new(NodeId::ZERO)?;
        let results = table.closest(&NodeId::ONE, 10, NonZeroU8::new(2).unwrap())?;
        assert!(results.is_empty());
        Ok(())
    }

    #[test]
    fn test_closest_multiple_buckets() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<2, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1..
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1..
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0..
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0..
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1110_0000);
        let results: Vec<_> = table
            .closest(&query_id, 10, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        // Closest to 1110... should be 1100... then 1000...
        // 1110... ^ 1100... = 0010... (prefix len 2)
        // 1110... ^ 1000... = 0110... (prefix len 1)
        // 1110... ^ 0100... = 1010... (prefix len 0)
        // 1110... ^ 0010... = 1100... (prefix len 0)

        assert_eq!(results.len(), 4);

        // 1100...
        assert_eq!(results[0].1, ids[1]);
        assert_eq!(results[0].0.bit_len(), 2);
        // 1000...
        assert_eq!(results[1].1, ids[0]);
        assert_eq!(results[1].0.bit_len(), 1);

        // shared prefix length 0
        assert_eq!(results[2].0.bit_len(), 0);
        assert_eq!(results[3].0.bit_len(), 0);

        // Sorting among same prefix length is undetermined
        //
        // unless the bucket of target if it is the last bucket
        // (not the case here)
        let pfx_zero: HashSet<_> = results[2..=3].iter().map(|(_, n)| n).collect();
        pfx_zero.contains(&ids[2]);
        pfx_zero.contains(&ids[3]);

        Ok(())
    }

    #[test]
    fn test_closest_single_bucket() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<2, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 1... consists of the two closest entries 1100... and 1000...
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(results.len(), 2);

        assert_eq!(results[0].1, ids[1]);
        assert_eq!(results[1].1, ids[0]);
        assert_eq!(results[0].0.bit_len(), 7);
        assert_eq!(results[1].0.bit_len(), 1);

        Ok(())
    }

    #[test]
    fn test_closest_single_bucket_select_same_prefix_progress() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<4, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1011_0000), // 0xb , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 1... has the closest entries 1100...
        // and two equally distanced entries 1000..., 1011... (some returned)
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(shared_prefix, contact)| (shared_prefix, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert!(results.len() >= 2, "return at least n nodes where possible");

        // shared prefix = 7 first 1100...
        assert_eq!(results[0].0.bit_len(), 7);
        assert_eq!(results[0].1, ids[2]);

        // shared prefix = 1 some(1000..., 1011...)
        for (sp, _) in results[1..].iter() {
            assert_eq!(sp.bit_len(), 1);
        }
        let result_prefix_one: HashSet<_> = results[1..].iter().map(|(_, n)| n).collect();
        assert_eq!(
            result_prefix_one.len(),
            results.len() - 1,
            "no duplicate node ids"
        );
        let prefix_one: HashSet<_> = ids[0..=1].iter().collect();
        assert!((result_prefix_one.is_subset(&prefix_one)));

        Ok(())
    }

    #[test]
    fn test_closest_single_bucket_select_single() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<4, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1011_0000), // 0xb , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        // Bucket 1... has the closest entries 1100...
        // and two equally distanced entries 1000..., 1011... (none returned)
        let query_id = NodeId::with_msb(0b1100_0001);
        let results: Vec<_> = table
            .closest(&query_id, 1, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert!(!results.is_empty());

        // shared prefix = 7 1100...
        assert_eq!(results[0].0.bit_len(), 7);
        assert_eq!(results[0].1, ids[2]);

        // Returning additional contacts isn't a problem as long as
        // the shared prefix length is less
        for (sp, n) in results[1..].iter() {
            assert!(
                sp.bit_len() < 2,
                "Should have prefix <2: [{n}: {}]",
                sp.bit_len()
            );
        }

        Ok(())
    }

    #[test]
    fn test_closest_single_bucket_select_last_bucket() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::with_msb(0xFF); // root changed to 1111...
        let mut table = FlatRoutingTable::<4, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1... <-- last bucket
            NodeId::with_msb(0b1011_0000), // 0xb , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 1... has the closest contact 1100...
        // and two contacts with the same prefix progress 1000..., 1011...
        // but 1000... is closer by XOR-metric
        // 1011... shouldn't be returned
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        // Problem if the other contact of the bucket (1011...) is returned
        // because it is further away by XOR-metric (XOR larger).
        // Proper proximity neighbor selection respecting the shared prefix length
        // could favor other contact
        assert_eq!(results.len(), 2);

        // 1100...
        assert_eq!(results[0].1, ids[2]);
        assert_eq!(results[0].0.bit_len(), 7);

        // 1000...
        assert_eq!(results[1].1, ids[0]);
        assert_eq!(results[1].0.bit_len(), 1);

        Ok(())
    }

    #[test]
    fn test_closest_down_tree() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<2, 1>::new(root)?;

        let ids = [
            // NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc, bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4, bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2, bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 1... has only one node but we want two nodes.
        // Closest should look into buckets down the tree
        // (deeper buckets) and return at least one contact
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert!(results.len() >= 2);

        assert_eq!(results[0].1, ids[0]);
        assert_eq!(results[0].0.bit_len(), 7);

        // check that (some) correct contacts from down the tree
        // are returned
        for (sp, _) in results[1..].iter() {
            assert!(sp.bit_len() < results[0].0.bit_len());
        }
        let result_down: HashSet<_> = results[1..].iter().map(|(_, n)| n).collect();
        let expected_down = ids[1..=2].iter().collect();
        assert!(
            result_down.is_subset(&expected_down),
            "Not part of expected nodes: {result_down:?} ⊈ {expected_down:?}"
        );

        // don't return duplicates
        assert_eq!(
            result_down.len(),
            results.len() - 1,
            "no duplicate node ids"
        );

        Ok(())
    }

    #[test]
    fn test_closest_up_tree() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<2, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            //NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b0100_0001);
        // Bucket 0... has only one node but we want two
        // Closest should look into buckets up the tree and return at least one contact
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert!(results.len() >= 2);

        // bucket entries are sorted correctly
        assert_eq!(results[0].1, ids[2]);
        assert_eq!(results[0].0.bit_len(), 1);

        // check that (some) correct contacts from up the tree are returned
        for (sp, _) in results[1..].iter() {
            assert!(sp.bit_len() < results[0].0.bit_len());
        }
        let result_up: HashSet<_> = results[1..].iter().map(|(_, n)| n).collect();
        let expected_up = ids[0..=1].iter().collect();
        assert!(
            result_up.is_subset(&expected_up),
            "Not part of expected nodes: {result_up:?} ⊈ {expected_up:?}"
        );

        // without any duplicates
        assert_eq!(result_up.len(), results.len() - 1, "no duplicate node ids");

        Ok(())
    }

    #[test]
    fn test_closest_invalid_contacts() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<4, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        *table.contact_mut(&ids[1]).unwrap().state_mut() =
            ContactState::Invalid(NotViaStateList::default());
        assert_eq!(
            table.contact(&ids[1]).unwrap().state(),
            &ContactState::Invalid(NotViaStateList::default()),
            "1100... is invalid",
        );

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 1... consists of the two closest contacts 1100... and 1000...
        // but shouldn't return 1100 because it is invalid
        let results: Vec<_> = table
            .closest(&query_id, 1, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(results.len(), 1);

        assert_eq!(results[0].1, ids[0]);
        assert_eq!(results[0].0.bit_len(), 1);

        Ok(())
    }

    #[test]
    fn test_closest_invalid_contacts_multiple_buckets() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<4, 1>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        *table.contact_mut(&ids[1]).unwrap().state_mut() =
            ContactState::Invalid(NotViaStateList::default());
        assert_eq!(
            table.contact(&ids[1]).unwrap().state(),
            &ContactState::Invalid(NotViaStateList::default()),
            "1100... is invalid",
        );

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 1... consists of the two closest entries 1100... and 1000...
        // but shouldn't return 1100 because it is invalid.
        // Closest has to look down the tree for another node.
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(results.len(), 2);

        assert_eq!(results[0].1, ids[0]);
        assert_eq!(results[0].0.bit_len(), 1);

        assert_ne!(results[1].1, ids[1]);
        assert_eq!(results[1].0.bit_len(), 0);

        Ok(())
    }

    #[test]
    fn test_closest_acc_same_level() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<1, 2>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 10...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 11...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 01...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 00...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 11... consists of 1100...
        // Closest should look at other bucket 10... (same level)
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert!(results.len() >= 2);

        // 1100...
        assert_eq!(results[0].1, ids[1]);
        assert_eq!(results[0].0.bit_len(), 7);

        // 1000...
        assert_eq!(results[1].1, ids[0]);
        assert_eq!(results[1].0.bit_len(), 1);

        // Returning additional contacts isn't a problem as long as
        // the shared prefix length is less
        for (sp, n) in results[2..].iter() {
            assert_eq!(sp.bit_len(), 0, "{n} should have prefix 0");
        }

        Ok(())
    }

    #[test]
    fn test_closest_acc_single_last_bucket() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<2, 2>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 10...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 11...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 01...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 00...
            NodeId::with_msb(0b0011_0000), // 0x3 , bucket 00...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b0000_0001);
        let results: Vec<_> = table
            .closest(&query_id, 1, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        // Problem if the other contact of the bucket (0011...) is returned
        // because it is further away by XOR-metric (XOR larger).
        // Proper proximity neighbor selection respecting the shared prefix length
        // could favor other contact.
        assert_eq!(results.len(), 1);

        // only return entry from last bucket 0010...
        assert_eq!(results[0].1, ids[3]);
        assert_eq!(results[0].0.bit_len(), 2);

        Ok(())
    }

    #[test]
    fn test_closest_acc_last_level() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<1, 2>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 10...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 11...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 01...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 00...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b0000_0001);
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert!(results.len() >= 2);

        // 0010...
        assert_eq!(results[0].1, ids[3]);
        assert_eq!(results[0].0.bit_len(), 2);

        // 0100...
        assert_eq!(results[1].1, ids[2]);
        assert_eq!(results[1].0.bit_len(), 1);

        // Returning additional contacts isn't a problem as long as
        // the shared prefix length is less
        for (sp, n) in results[2..].iter() {
            assert_eq!(sp.bit_len(), 0, "{n} should have prefix 0");
        }

        Ok(())
    }

    #[test]
    fn test_closest_acc_multilevel() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<1, 2>::new(root)?;

        let ids = [
            NodeId::with_msb(0b1000_0000), // bucket 10...
            NodeId::with_msb(0b1100_0000), // bucket 11...
            NodeId::with_msb(0b0100_0000), // bucket 01...
            NodeId::with_msb(0b0010_0000), // bucket 0010...
            NodeId::with_msb(0b0011_0000), // bucket 0011...
            // bucket 0001...
            NodeId::with_msb(0b0000_0001), // bucket 0000...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1000_0001);
        // Bucket 11... consists of 1100...
        // Closest should look at other buckets of the same depth
        let results: Vec<_> = table
            .closest(&query_id, 4, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert!(results.len() >= 4);

        // 1000...
        assert_eq!(results[0].1, ids[0]);
        assert_eq!(results[0].0.bit_len(), 7);

        // 1100...
        assert_eq!(results[1].1, ids[1]);
        assert_eq!(results[1].0.bit_len(), 1);

        // ordering undetermined because all shared prefix = 0

        let result_zero: HashSet<_> = results[2..].iter().map(|(_, n)| n).collect();
        let expected_zero = ids[2..].iter().collect();
        assert!(
            result_zero.is_subset(&expected_zero),
            "Not part of expected nodes: {result_zero:?} ⊈ {expected_zero:?}"
        );

        // don't return duplicates
        assert_eq!(
            result_zero.len(),
            results.len() - 2,
            "no duplicate node ids of shared prefix = 0: {result_zero:?}"
        );

        Ok(())
    }

    #[test]
    fn test_closest_n_larger_than_total() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<10, 1>::new(root)?;

        let id = NodeId::with_msb(0b1000_0000);
        table.insert(Contact::new(
            Path::from(id),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        let query_id = NodeId::with_msb(0b1111_0000);
        let results: Vec<_> = table
            .closest(&query_id, 10, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(results.len(), 1);

        assert_eq!(results[0].1, id);
        assert_eq!(results[0].0.bit_len(), 1);

        Ok(())
    }

    #[test]
    fn test_closest_different_grouping() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = FlatRoutingTable::<10, 1>::new(root)?;

        let id = NodeId::with_msb(0b1100_0000);
        table.insert(Contact::new(
            Path::from(id),
            SafeStateSeqNr::try_from(1).unwrap(),
        ))?;

        let query_id = NodeId::with_msb(0b1000_0000);

        // Grouping 1: prefix len is 1 (1...)
        let results = table.closest(&query_id, 10, NonZeroU8::new(1).unwrap())?;
        assert_eq!(results[0].0.bit_len(), 1);
        assert_eq!(results.len(), 1);

        // Grouping 2: prefix len is 0 (first 2 bits are 11 vs 10, so first group of 2 bits doesn't match)
        let results = table.closest(&query_id, 10, NonZeroU8::new(2).unwrap())?;
        assert_eq!(results[0].0.bit_len(), 0);
        assert_eq!(results.len(), 1);

        let query_id = NodeId::with_msb(0b1100_1000);

        // Grouping 1: prefix len is 4 (1100...)
        let results = table.closest(&query_id, 10, NonZeroU8::new(1).unwrap())?;
        assert_eq!(results[0].0.bit_len(), 4);
        assert_eq!(results.len(), 1);

        // Grouping 2: prefix len is 2 (11_00...)
        let results = table.closest(&query_id, 10, NonZeroU8::new(2).unwrap())?;
        assert_eq!(results[0].0.bit_len(), 2);
        assert_eq!(results.len(), 1);

        // Grouping 3: prefix len is 1 (110....)
        let results = table.closest(&query_id, 10, NonZeroU8::new(3).unwrap())?;
        assert_eq!(results[0].0.bit_len(), 1);
        assert_eq!(results.len(), 1);

        Ok(())
    }

    #[test]
    fn test_closest_one_bucket_minimal_topo() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        const BUCKET_SIZE: usize = 20;
        let nodes = crate::tests::minimal_topo_nodes();
        assert!(BUCKET_SIZE >= nodes.len() - 1); // should fit all into one bucket

        for root in nodes {
            // put all other nodes inside FlatRoutingTable
            let mut table = FlatRoutingTable::<BUCKET_SIZE, 1>::new(root)?;
            for node_id in nodes {
                if node_id != root {
                    table
                        .add(Contact::new(
                            Path::from(node_id),
                            SafeStateSeqNr::try_from(1).unwrap(),
                        ))
                        .expect("failed to add node to rt without splitting");
                }
            }

            for node_id in nodes.iter() {
                // return all nodes on closest
                let results = table.closest(node_id, BUCKET_SIZE, NonZeroU8::new(1).unwrap())?;
                assert_eq!(results.len(), nodes.len() - 1, "return all nodes");

                if node_id != &root {
                    assert_eq!(results[0].1.id(), node_id);
                }

                assert!(
                    results.is_sorted_by(sorted_xor),
                    "last bucket is not sorted by XOR-metric"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn test_closest_small_bucket_minimal_topo_0() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let nodes = crate::tests::minimal_topo_nodes();
        let root = nodes[0];
        let mut table = FlatRoutingTable::<3, 1>::new(root)?; // BUCKET_SIZE = 3

        // put all other nodes inside FlatRoutingTable
        for node_id in nodes {
            if node_id != root
                && let Err(e) = table.insert(Contact::new(
                    Path::from(node_id),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
            {
                tracing::info!(reason = %e, node = %node_id, "Failed to add node");
            }
        }
        let last_bucket_index = table.buckets.len() - 1;

        // last bucket: k2, k4, k16
        assert_eq!(table.get_bucket_index(&nodes[2]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[4]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[16]), last_bucket_index);

        let mut expected_table_nodes = HashSet::from(nodes);
        expected_table_nodes.remove(&root);
        // nodes that can't be added because their bucket is full and unsplittable
        expected_table_nodes.remove(&nodes[10]);
        expected_table_nodes.remove(&nodes[11]);
        expected_table_nodes.remove(&nodes[12]);
        expected_table_nodes.remove(&nodes[15]);
        expected_table_nodes.remove(&nodes[17]);
        expected_table_nodes.remove(&nodes[18]);
        expected_table_nodes.remove(&nodes[19]);
        let expected_table_nodes = expected_table_nodes;

        // check expected nodes are in routing table
        let table_nodes: HashSet<_> = table.iter().map(|contact| *contact.id()).collect();
        assert_eq!(
            table_nodes, expected_table_nodes,
            "Routing Table contains unexpected node",
        );
        assert_eq!(
            table_nodes.len(),
            expected_table_nodes.len(),
            "Duplicate nodes in Routing Table: {table_nodes:?}"
        );

        // QUERY: ROOT
        let query_id = root;
        let results: Vec<_> = table
            .closest(&query_id, 4, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 4);

        // SP=5: k4
        assert_eq!(&results[0].1, &nodes[4]);
        assert_eq!(results[0].0.bit_len(), 5);

        // SP=4: k16, k2 (order determined by XOR-metric because of last bucket)

        // k16, XOR: 0x008c..., SP=4
        assert_eq!(&results[1].1, &nodes[16]);
        assert_eq!(results[1].0.bit_len(), 4);

        // k2, XOR: 0x008f..., SP=4
        assert_eq!(&results[2].1, &nodes[2]);
        assert_eq!(results[2].0.bit_len(), 4);

        // SP=3 k1, k3, k7
        assert_eq!(results[3].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[1], nodes[3], nodes[7]]);
        let result_three: HashSet<_> = results[3..=5].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // QUERY: k9
        let query_id = nodes[9];
        let results: Vec<_> = table
            .closest(&query_id, 3, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 3);

        // SP=112: k9 (query node itself)
        assert_eq!(results[0].1, nodes[9]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS);

        // SP=5: k5
        assert_eq!(results[1].1, nodes[5]);
        assert_eq!(results[1].0.bit_len(), 5);

        // SP=2: k6, [k10, k17]
        // only k6 is in the Routing Table and should be returned
        assert_eq!(results[2].1, nodes[6]);
        assert_eq!(results[2].0.bit_len(), 2);

        // SP=1: [k11, k12]

        // QUERY: k1 ^ 1
        let query_id = nodes[1] ^ NodeId::ONE;
        let results: Vec<_> = table
            .closest(&query_id, 5, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 5);

        // SP=111: k1 (one bit difference because of XOR ONE)
        assert_eq!(results[0].1, nodes[1]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS - 1);

        // SP=8: k3
        assert_eq!(results[1].1, nodes[3]);
        assert_eq!(results[1].0.bit_len(), 8);

        // SP=4: k7
        assert_eq!(results[2].1, nodes[7]);
        assert_eq!(results[2].0.bit_len(), 4);

        // SP=3: [k0], k2, k4, k16
        assert_eq!(results[3].0.bit_len(), 3);
        assert_eq!(results[4].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[2], nodes[4], nodes[16]]);
        let result_three: HashSet<_> = results[3..=4].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        Ok(())
    }

    #[test]
    fn test_closest_small_bucket_minimal_topo_0_acc() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let nodes = crate::tests::minimal_topo_nodes();
        let root = nodes[0];
        let mut table = FlatRoutingTable::<3, 2>::new(root)?; // BUCKET_SIZE = 3, ACC = 2

        // put all other nodes inside FlatRoutingTable
        for node_id in nodes {
            if node_id != root
                && let Err(e) = table.insert(Contact::new(
                    Path::from(node_id),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
            {
                tracing::info!(reason = %e, node = %node_id, "Failed to add node");
            }
        }
        let last_bucket_index = table.buckets.len() - 1;

        // last bucket: k2, k4, k16
        assert_eq!(table.get_bucket_index(&nodes[2]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[4]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[16]), last_bucket_index);

        let mut expected_table_nodes = HashSet::from(nodes);
        expected_table_nodes.remove(&root);
        // nodes that can't be added because their bucket is full and unsplittable
        expected_table_nodes.remove(&nodes[10]);
        expected_table_nodes.remove(&nodes[15]);
        expected_table_nodes.remove(&nodes[17]);
        expected_table_nodes.remove(&nodes[18]);
        expected_table_nodes.remove(&nodes[19]);
        let expected_table_nodes = expected_table_nodes;

        // check expected nodes are in routing table
        let table_nodes: HashSet<_> = table.iter().map(|contact| *contact.id()).collect();
        assert_eq!(
            table_nodes, expected_table_nodes,
            "Routing Table contains unexpected node",
        );
        assert_eq!(
            table_nodes.len(),
            expected_table_nodes.len(),
            "Duplicate nodes in Routing Table: {table_nodes:?}"
        );

        // QUERY: ROOT
        let query_id = root;
        let results: Vec<_> = table
            .closest(&query_id, 4, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 4);

        // SP=5: k4
        assert_eq!(&results[0].1, &nodes[4]);
        assert_eq!(results[0].0.bit_len(), 5);

        // SP=4: k16, k2 (order determined by XOR-metric because of last bucket)

        // k16, XOR: 0x008c..., SP=4
        assert_eq!(&results[1].1, &nodes[16]);
        assert_eq!(results[1].0.bit_len(), 4);

        // k2, XOR: 0x008f..., SP=4
        assert_eq!(&results[2].1, &nodes[2]);
        assert_eq!(results[2].0.bit_len(), 4);

        // SP=3 k1, k3, k7
        assert_eq!(results[3].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[1], nodes[3], nodes[7]]);
        let result_three: HashSet<_> = results[3..=5].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // QUERY: k9
        let query_id = nodes[9];
        let results: Vec<_> = table
            .closest(&query_id, 3, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 3);

        // SP=112: k9 (query node itself)
        assert_eq!(results[0].1, nodes[9]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS);

        // SP=5: k5
        assert_eq!(results[1].1, nodes[5]);
        assert_eq!(results[1].0.bit_len(), 5);

        // SP=2: k6, [k10, k17]
        // only k6 is in the Routing Table and should be returned
        assert_eq!(results[2].1, nodes[6]);
        assert_eq!(results[2].0.bit_len(), 2);

        // SP=1: k11, k12

        // QUERY: k1 ^ 1
        let query_id = nodes[1] ^ NodeId::ONE;
        let results: Vec<_> = table
            .closest(&query_id, 5, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 5);

        // SP=111: k1 (one bit difference because of XOR ONE)
        assert_eq!(results[0].1, nodes[1]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS - 1);

        // SP=8: k3
        assert_eq!(results[1].1, nodes[3]);
        assert_eq!(results[1].0.bit_len(), 8);

        // SP=4: k7
        assert_eq!(results[2].1, nodes[7]);
        assert_eq!(results[2].0.bit_len(), 4);

        // SP=3: [k0], k2, k4, k16
        assert_eq!(results[3].0.bit_len(), 3);
        assert_eq!(results[4].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[2], nodes[4], nodes[16]]);
        let result_three: HashSet<_> = results[3..=4].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        Ok(())
    }

    #[test]
    fn test_closest_small_bucket_minimal_topo_19() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let nodes = crate::tests::minimal_topo_nodes();
        let root = nodes[19];
        let mut table = FlatRoutingTable::<3, 1>::new(root)?; // BUCKET_SIZE = 3

        // put all other nodes inside FlatRoutingTable
        for node_id in nodes {
            if node_id != root
                && let Err(e) = table.insert(Contact::new(
                    Path::from(node_id),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
            {
                tracing::info!(reason = %e, node = %node_id, "Failed to add node");
            }
        }
        let last_bucket_index = table.buckets.len() - 1;

        // last bucket: k8, k13, 18
        assert_eq!(table.get_bucket_index(&nodes[8]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[13]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[18]), last_bucket_index);

        let mut expected_table_nodes = HashSet::from(nodes);
        expected_table_nodes.remove(&root);
        // nodes that can't be added because their bucket is full and unsplittable
        expected_table_nodes.remove(&nodes[3]);
        expected_table_nodes.remove(&nodes[4]);
        expected_table_nodes.remove(&nodes[7]);
        expected_table_nodes.remove(&nodes[10]);
        expected_table_nodes.remove(&nodes[11]);
        expected_table_nodes.remove(&nodes[12]);
        expected_table_nodes.remove(&nodes[16]);
        expected_table_nodes.remove(&nodes[17]);
        let expected_table_nodes = expected_table_nodes;

        // check expected nodes are in routing table
        let table_nodes: HashSet<_> = table.iter().map(|contact| *contact.id()).collect();
        assert_eq!(
            table_nodes, expected_table_nodes,
            "Routing Table contains unexpected node",
        );
        assert_eq!(
            table_nodes.len(),
            expected_table_nodes.len(),
            "Duplicate nodes in Routing Table: {table_nodes:?}"
        );

        // QUERY: ROOT
        let query_id = root;
        let results: Vec<_> = table
            .closest(&query_id, 4, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 4);

        // SP=6: k13
        assert_eq!(&results[0].1, &nodes[13]);
        assert_eq!(results[0].0.bit_len(), 6);

        // SP=3: k8, k18
        assert_eq!(results[1].0.bit_len(), 3);
        assert_eq!(results[2].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[8], nodes[18]]);
        let result_three: HashSet<_> = results[1..=2].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // SP=2: k14, k15
        assert_eq!(results[3].0.bit_len(), 2);
        assert_eq!(results[4].0.bit_len(), 2);
        let expected_two = HashSet::from([nodes[14], nodes[15]]);
        let result_two: HashSet<_> = results[3..=4].iter().map(|(_, n)| *n).collect();
        assert!(
            result_two.is_subset(&expected_two),
            "Unexpected node for SP=2: {result_two:?} ⊈ {expected_two:?}"
        );

        // QUERY: k9
        let query_id = nodes[9];
        let results: Vec<_> = table
            .closest(&query_id, 3, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 3);

        // SP=112: k9 (query node itself)
        assert_eq!(results[0].1, nodes[9]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS);

        // SP=5: k5
        assert_eq!(results[1].1, nodes[5]);
        assert_eq!(results[1].0.bit_len(), 5);

        // SP=2: k6, [k10, k17]
        // only k6 is in the Routing Table and should be returned
        assert_eq!(results[2].1, nodes[6]);
        assert_eq!(results[2].0.bit_len(), 2);

        // SP=1: [k11, k12]

        // any shared prefix length 0 candidate should be returned
        for (sp, n) in results[3..].iter() {
            assert_eq!(sp.bit_len(), 0, "{n} has shared prefix > 0: {sp:?}");
        }

        // QUERY: k1 ^ 1
        let query_id = nodes[1] ^ NodeId::ONE;
        let results: Vec<_> = table
            .closest(&query_id, 5, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 5);

        // SP=111: k1 (one bit difference because of XOR ONE)
        assert_eq!(results[0].1, nodes[1]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS - 1);

        // SP=8: [k3]

        // SP=4: [k7]

        // SP=3: k0, k2, [k4], [k16]
        assert_eq!(results[1].0.bit_len(), 3);
        assert_eq!(results[2].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[0], nodes[2]]);
        let result_three: HashSet<_> = results[1..=2].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // SP=1: k8, k13, k14, k15, k18, k19
        assert_eq!(results[3].0.bit_len(), 1);
        assert_eq!(results[4].0.bit_len(), 1);
        let expected_two = HashSet::from([
            nodes[8], nodes[13], nodes[14], nodes[15], nodes[18], nodes[19],
        ]);
        let result_two: HashSet<_> = results[3..].iter().map(|(_, n)| *n).collect();
        assert!(
            result_two.is_subset(&expected_two),
            "Unexpected node for SP=3: {result_two:?} ⊈ {expected_two:?}"
        );

        Ok(())
    }

    #[test]
    fn test_closest_small_bucket_minimal_topo_19_acc() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let nodes = crate::tests::minimal_topo_nodes();
        let root = nodes[19];
        let mut table = FlatRoutingTable::<3, 2>::new(root)?; // BUCKET_SIZE = 3, ACC = 2

        // put all other nodes inside FlatRoutingTable
        for node_id in nodes {
            if node_id != root
                && let Err(e) = table.insert(Contact::new(
                    Path::from(node_id),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
            {
                tracing::info!(reason = %e, node = %node_id, "Failed to add node");
            }
        }
        let last_bucket_index = table.buckets.len() - 1;

        // last bucket: k13
        assert_eq!(table.get_bucket_index(&nodes[13]), last_bucket_index);

        let mut expected_table_nodes = HashSet::from(nodes);
        expected_table_nodes.remove(&root);
        // nodes that can't be added because their bucket is full and unsplittable
        expected_table_nodes.remove(&nodes[3]);
        expected_table_nodes.remove(&nodes[4]);
        expected_table_nodes.remove(&nodes[7]);
        expected_table_nodes.remove(&nodes[10]);
        expected_table_nodes.remove(&nodes[16]);
        expected_table_nodes.remove(&nodes[17]);
        let expected_table_nodes = expected_table_nodes;

        // check expected nodes are in routing table
        let table_nodes: HashSet<_> = table.iter().map(|contact| *contact.id()).collect();
        assert_eq!(
            table_nodes, expected_table_nodes,
            "Routing Table contains unexpected node",
        );
        assert_eq!(
            table_nodes.len(),
            expected_table_nodes.len(),
            "Duplicate nodes in Routing Table: {table_nodes:?}"
        );

        // QUERY: ROOT
        let query_id = root;
        let results: Vec<_> = table
            .closest(&query_id, 4, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 4);

        // SP=6: k13
        assert_eq!(&results[0].1, &nodes[13]);
        assert_eq!(results[0].0.bit_len(), 6);

        // SP=3: k8, k18
        assert_eq!(results[1].0.bit_len(), 3);
        assert_eq!(results[2].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[8], nodes[18]]);
        let result_three: HashSet<_> = results[1..=2].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // SP=2: k14, k15
        assert_eq!(results[3].0.bit_len(), 2);
        assert_eq!(results[4].0.bit_len(), 2);
        let expected_two = HashSet::from([nodes[14], nodes[15]]);
        let result_two: HashSet<_> = results[3..=4].iter().map(|(_, n)| *n).collect();
        assert!(
            result_two.is_subset(&expected_two),
            "Unexpected node for SP=2: {result_two:?} ⊈ {expected_two:?}"
        );

        // QUERY: k9
        let query_id = nodes[9];
        let results: Vec<_> = table
            .closest(&query_id, 3, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 3);

        // SP=112: k9 (query node itself)
        assert_eq!(results[0].1, nodes[9]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS);

        // SP=5: k5
        assert_eq!(results[1].1, nodes[5]);
        assert_eq!(results[1].0.bit_len(), 5);

        // SP=2: k6, [k10, k17]
        // only k6 is in the Routing Table and should be returned
        assert_eq!(results[2].1, nodes[6]);
        assert_eq!(results[2].0.bit_len(), 2);

        // SP=1: [k11, k12]

        // any shared prefix length 0 candidate should be returned
        for (sp, n) in results[3..].iter() {
            assert_eq!(sp.bit_len(), 0, "{n} has shared prefix > 0: {sp:?}");
        }

        // QUERY: k1 ^ 1
        let query_id = nodes[1] ^ NodeId::ONE;
        let results: Vec<_> = table
            .closest(&query_id, 5, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 5);

        // SP=111: k1 (one bit difference because of XOR ONE)
        assert_eq!(results[0].1, nodes[1]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS - 1);

        // SP=8: [k3]

        // SP=4: [k7]

        // SP=3: k0, k2, [k4], [k16]
        assert_eq!(results[1].0.bit_len(), 3);
        assert_eq!(results[2].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[0], nodes[2]]);
        let result_three: HashSet<_> = results[1..=2].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // SP=1: k8, k13, k14, k15, k18, k19
        assert_eq!(results[3].0.bit_len(), 1);
        let expected_one = HashSet::from([
            nodes[8], nodes[13], nodes[14], nodes[15], nodes[18], nodes[19],
        ]);
        let result_one: HashSet<_> = results[3..].iter().map(|(_, n)| *n).collect();
        assert!(
            result_one.is_subset(&expected_one),
            "Unexpected node for SP=3: {result_one:?} ⊈ {expected_one:?}"
        );

        Ok(())
    }

    #[test]
    fn test_closest_small_bucket_minimal_topo_10() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let nodes = crate::tests::minimal_topo_nodes();
        let root = nodes[10];
        let mut table = FlatRoutingTable::<3, 1>::new(root)?;

        // put all other nodes inside FlatRoutingTable
        for node_id in nodes {
            if node_id != root
                && let Err(e) = table.insert(Contact::new(
                    Path::from(node_id),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
            {
                tracing::info!(reason = %e, node = %node_id, "Failed to add node");
            }
        }
        let last_bucket_index = table.buckets.len() - 1;

        // last bucket: k6, k17
        assert_eq!(table.get_bucket_index(&nodes[6]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[17]), last_bucket_index);
        println!("{:#?}", table.buckets[last_bucket_index]);

        let mut expected_table_nodes = HashSet::from(nodes);
        expected_table_nodes.remove(&root);
        // nodes that can't be added because their bucket is full and unsplittable
        expected_table_nodes.remove(&nodes[3]);
        expected_table_nodes.remove(&nodes[4]);
        expected_table_nodes.remove(&nodes[7]);
        expected_table_nodes.remove(&nodes[8]);
        expected_table_nodes.remove(&nodes[13]);
        expected_table_nodes.remove(&nodes[14]);
        expected_table_nodes.remove(&nodes[15]);
        expected_table_nodes.remove(&nodes[16]);
        expected_table_nodes.remove(&nodes[18]);
        expected_table_nodes.remove(&nodes[19]);
        let expected_table_nodes = expected_table_nodes;

        // check expected nodes are in routing table
        let table_nodes: HashSet<_> = table.iter().map(|contact| *contact.id()).collect();
        assert_eq!(
            table_nodes, expected_table_nodes,
            "Routing Table contains unexpected node",
        );
        assert_eq!(
            table_nodes.len(),
            expected_table_nodes.len(),
            "Duplicate nodes in Routing Table: {table_nodes:?}"
        );

        // QUERY: ROOT
        let query_id = root;
        let results: Vec<_> = table
            .closest(&query_id, 4, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 4);

        // SP=5: k17
        assert_eq!(&results[0].1, &nodes[17]);
        assert_eq!(results[0].0.bit_len(), 5);

        // SP=4: k13
        assert_eq!(&results[1].1, &nodes[6]);
        assert_eq!(results[1].0.bit_len(), 4);

        // SP=3: k5, k9
        assert_eq!(results[2].0.bit_len(), 2);
        assert_eq!(results[3].0.bit_len(), 2);
        let expected_three = HashSet::from([nodes[5], nodes[9]]);
        let result_three: HashSet<_> = results[2..=3].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // QUERY: k9
        let query_id = nodes[9];
        let results: Vec<_> = table
            .closest(&query_id, 8, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 8);

        // SP=112: k9 (query node itself)
        assert_eq!(results[0].1, nodes[9]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS);

        // SP=5: k5
        assert_eq!(results[1].1, nodes[5]);
        assert_eq!(results[1].0.bit_len(), 5);

        // SP=2: k6, [k10], k17
        assert_eq!(results[2].0.bit_len(), 2);
        assert_eq!(results[3].0.bit_len(), 2);
        let expected_two = HashSet::from([nodes[6], nodes[17]]);
        let result_three: HashSet<_> = results[2..=3].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_two),
            "Unexpected node for SP=2: {result_three:?} ⊈ {expected_two:?}"
        );

        // SP=1: k11, k12
        assert_eq!(results[4].0.bit_len(), 1);
        assert_eq!(results[5].0.bit_len(), 1);
        let expected_two = HashSet::from([nodes[11], nodes[12]]);
        let result_three: HashSet<_> = results[4..=5].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_two),
            "Unexpected node for SP=2: {result_three:?} ⊈ {expected_two:?}"
        );

        // any two shared prefix length 0 candidate should be returned
        for (sp, n) in results[6..].iter() {
            assert_eq!(sp.bit_len(), 0, "{n} has shared prefix > 0: {sp:?}");
        }

        // QUERY: k1 ^ 1
        let query_id = nodes[1] ^ NodeId::ONE;
        let results: Vec<_> = table
            .closest(&query_id, 5, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 5);

        // SP=111: k1 (one bit difference because of XOR ONE)
        assert_eq!(results[0].1, nodes[1]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS - 1);

        // SP=8: [k3]

        // SP=4: [k7]

        // SP=3: k0, k2, [k4], [k16]
        assert_eq!(results[1].0.bit_len(), 3);
        assert_eq!(results[2].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[0], nodes[2]]);
        let result_three: HashSet<_> = results[1..=2].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // SP=1: [k8, k13, k14, k15, k18, k19]

        // any three shared prefix length 0 candidate should be returned
        for (sp, n) in results[3..].iter() {
            assert_eq!(sp.bit_len(), 0, "{n} has shared prefix > 0: {sp:?}");
        }

        Ok(())
    }

    #[test]
    fn test_closest_small_bucket_minimal_topo_10_acc() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let nodes = crate::tests::minimal_topo_nodes();
        let root = nodes[10];
        let mut table = FlatRoutingTable::<3, 2>::new(root)?; // BUCKET_SIZE = 3, ACC = 2

        // put all other nodes inside FlatRoutingTable
        for node_id in nodes {
            if node_id != root
                && let Err(e) = table.insert(Contact::new(
                    Path::from(node_id),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
            {
                tracing::info!(reason = %e, node = %node_id, "Failed to add node");
            }
        }
        let last_bucket_index = table.buckets.len() - 1;

        // last bucket: k6, k17
        assert_eq!(table.get_bucket_index(&nodes[6]), last_bucket_index);
        assert_eq!(table.get_bucket_index(&nodes[17]), last_bucket_index);
        println!("{:#?}", table.buckets[last_bucket_index]);

        let mut expected_table_nodes = HashSet::from(nodes);
        expected_table_nodes.remove(&root);
        // nodes that can't be added because their bucket is full and unsplittable
        expected_table_nodes.remove(&nodes[3]);
        expected_table_nodes.remove(&nodes[4]);
        expected_table_nodes.remove(&nodes[7]);
        expected_table_nodes.remove(&nodes[15]);
        expected_table_nodes.remove(&nodes[16]);
        expected_table_nodes.remove(&nodes[18]);
        expected_table_nodes.remove(&nodes[19]);
        let expected_table_nodes = expected_table_nodes;

        // check expected nodes are in routing table
        let table_nodes: HashSet<_> = table.iter().map(|contact| *contact.id()).collect();
        assert_eq!(
            table_nodes, expected_table_nodes,
            "Routing Table contains unexpected node",
        );
        assert_eq!(
            table_nodes.len(),
            expected_table_nodes.len(),
            "Duplicate nodes in Routing Table: {table_nodes:?}"
        );

        // QUERY: ROOT
        let query_id = root;
        let results: Vec<_> = table
            .closest(&query_id, 4, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 4);

        // SP=5: k17
        assert_eq!(&results[0].1, &nodes[17]);
        assert_eq!(results[0].0.bit_len(), 5);

        // SP=4: k13
        assert_eq!(&results[1].1, &nodes[6]);
        assert_eq!(results[1].0.bit_len(), 4);

        // SP=3: k5, k9
        assert_eq!(results[2].0.bit_len(), 2);
        assert_eq!(results[3].0.bit_len(), 2);
        let expected_three = HashSet::from([nodes[5], nodes[9]]);
        let result_three: HashSet<_> = results[2..=3].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // QUERY: k9
        let query_id = nodes[9];
        let results: Vec<_> = table
            .closest(&query_id, 8, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 8);

        // SP=112: k9 (query node itself)
        assert_eq!(results[0].1, nodes[9]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS);

        // SP=5: k5
        assert_eq!(results[1].1, nodes[5]);
        assert_eq!(results[1].0.bit_len(), 5);

        // SP=2: k6, [k10], k17
        assert_eq!(results[2].0.bit_len(), 2);
        assert_eq!(results[3].0.bit_len(), 2);
        let expected_two = HashSet::from([nodes[6], nodes[17]]);
        let result_three: HashSet<_> = results[2..=3].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_two),
            "Unexpected node for SP=2: {result_three:?} ⊈ {expected_two:?}"
        );

        // SP=1: k11, k12
        assert_eq!(results[4].0.bit_len(), 1);
        assert_eq!(results[5].0.bit_len(), 1);
        let expected_two = HashSet::from([nodes[11], nodes[12]]);
        let result_three: HashSet<_> = results[4..=5].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_two),
            "Unexpected node for SP=2: {result_three:?} ⊈ {expected_two:?}"
        );

        // any two shared prefix length 0 candidate should be returned
        for (sp, n) in results[6..].iter() {
            assert_eq!(sp.bit_len(), 0, "{n} has shared prefix > 0: {sp:?}");
        }

        // QUERY: k1 ^ 1
        let query_id = nodes[1] ^ NodeId::ONE;
        let results: Vec<_> = table
            .closest(&query_id, 5, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");
        assert!(results.len() >= 5);

        // SP=111: k1 (one bit difference because of XOR ONE)
        assert_eq!(results[0].1, nodes[1]);
        assert_eq!(results[0].0.bit_len(), NodeId::BITS - 1);

        // SP=8: [k3]

        // SP=4: [k7]

        // SP=3: k0, k2, [k4], [k16]
        assert_eq!(results[1].0.bit_len(), 3);
        assert_eq!(results[2].0.bit_len(), 3);
        let expected_three = HashSet::from([nodes[0], nodes[2]]);
        let result_three: HashSet<_> = results[1..=2].iter().map(|(_, n)| *n).collect();
        assert!(
            result_three.is_subset(&expected_three),
            "Unexpected node for SP=3: {result_three:?} ⊈ {expected_three:?}"
        );

        // SP=1: k8, k13, k14, [k15, k18, k19]
        assert_eq!(results[3].0.bit_len(), 1);
        assert_eq!(results[4].0.bit_len(), 1);
        let expected_one = HashSet::from([nodes[8], nodes[13], nodes[14]]);
        let result_one: HashSet<_> = results[3..=4].iter().map(|(_, n)| *n).collect();
        assert!(
            result_one.is_subset(&expected_one),
            "Unexpected node for SP=3: {result_one:?} ⊈ {expected_one:?}"
        );

        Ok(())
    }
}
