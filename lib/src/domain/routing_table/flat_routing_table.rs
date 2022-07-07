use std::num::NonZeroUsize;

use rand::Rng;

use crate::domain::{
    node_id, AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, GroupingError,
    NodeId, ReplacementError, RoutingTable, SharedPrefix, DEFAULT_BUCKET_SIZE,
};

pub const DEFAULT_ACCELERATION: usize = 1;

/// A [RoutingTable] implemented as flat array of [Bucket]s.
///
/// This [RoutingTable] has a root [NodeId] which the distance is computed to.
///
/// This kind of [RoutingTable] is only working with Physical Neighbor Selection
/// and Proximity Routing.
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
        let mut buckets = Vec::with_capacity(node_id::BIT_SIZE);
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

pub struct Iter<'a, const BUCKET_SIZE: usize, const ACC: usize> {
    table: &'a FlatRoutingTable<BUCKET_SIZE, ACC>,
    index: (usize, usize),
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> Iter<'a, BUCKET_SIZE, ACC> {
    fn new(table: &'a FlatRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        Self {
            table,
            index: (0, 0),
        }
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> Iterator for Iter<'a, BUCKET_SIZE, ACC> {
    type Item = &'a Contact;

    fn next(&mut self) -> Option<Self::Item> {
        // Return none if no further bucket is present
        let bucket = self.table.buckets.get(self.index.0)?;
        match bucket.get_by_index(self.index.1) {
            // Found a contact, go to next contact
            Some(contact) => {
                self.index.1 += 1;
                Some(contact)
            }
            // Found no contact in this bucket. Go to next
            None => {
                self.index.0 += 1;
                None
            }
        }
    }
}

impl<const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<BUCKET_SIZE>
    for FlatRoutingTable<BUCKET_SIZE, ACC>
{
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

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<(), ReplacementError> {
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
        self.into_iter()
            .nth(random_contact)
            .map(|contact| contact.id())
    }

    fn contact_mut(&mut self, id: &NodeId) -> Option<&mut Contact> {
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

    fn bucket_mut(&mut self, of: &NodeId) -> &mut Bucket<BUCKET_SIZE> {
        let index = self.get_bucket_index(of);
        &mut self.buckets[index]
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> IntoIterator
    for &'a FlatRoutingTable<BUCKET_SIZE, ACC>
{
    type Item = &'a Contact;

    type IntoIter = Iter<'a, BUCKET_SIZE, ACC>;

    fn into_iter(self) -> Self::IntoIter {
        Iter::new(self)
    }
}

#[cfg(test)]
mod routing_tests {
    use std::error::Error;

    use crate::domain::{
        AddError, Age, Contact, FlatRoutingTable, NodeId, Path, RoutingTable, StateSeqNr,
    };

    #[test]
    fn test_add() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        let contact = Contact::new(Path::from(NodeId::one()), Age::from(1), StateSeqNr::from(1));

        assert_eq!(table.add(contact), Ok(()));

        Ok(())
    }

    #[test]
    fn test_add_full() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.add(Contact::new(
            Path::from(NodeId::with_lsb(1)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        assert_eq!(
            table.add(Contact::new(
                Path::from(NodeId::with_lsb(2)),
                Age::from(0),
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

        table.add(Contact::new(
            Path::from(NodeId::one()),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        assert_eq!(table.split_bucket(&NodeId::one()), Ok(()));

        assert_eq!(table.get_bucket_index(&NodeId::one()), 1);

        Ok(())
    }

    #[test]
    fn test_insert_to_max_buckets() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1>::new(NodeId::zero())?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000001)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000010)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00000100)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00001000)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00010000)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b00100000)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b01000000)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        table.insert(Contact::new(
            Path::from(NodeId::with_lsb(0b10000000)),
            Age::from(0),
            StateSeqNr::from(0),
        ))?;

        assert!(table
            .insert(Contact::new(
                Path::from(NodeId::with_lsb(0b00010111)),
                Age::from(0),
                StateSeqNr::from(0),
            ))
            .is_err());
        assert_eq!(table.num_buckets(), FlatRoutingTable::<1, 1>::max_buckets());

        Ok(())
    }
}
