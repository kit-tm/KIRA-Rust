use std::num::NonZeroUsize;

use rand::Rng;

use crate::domain::{
    AddError, Bucket, BucketSplitError, Contact, GroupingError, NodeId, ReplacementError,
    RoutingTable, SharedPrefix, DEFAULT_BUCKET_SIZE,
};

/// A [RoutingTable] implemented as flat array of [Bucket]s.
///
/// This [RoutingTable] has a root [NodeId] which the distance is computed to.
///
/// This kind of [RoutingTable] is only working with Physical Neighbor Selection
/// and Proximity Routing.
#[derive(Debug)]
pub struct FlatRoutingTable<
    const ID_SIZE: usize,
    const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE,
    const ACC: usize = 1,
> {
    buckets: Vec<Bucket<ID_SIZE, BUCKET_SIZE>>,
    root: NodeId<ID_SIZE>,
}

impl<const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize>
    FlatRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>
{
    /// Creates a [RoutingTable] with 0 capacity.
    pub fn new(root: NodeId<ID_SIZE>) -> Result<Self, GroupingError> {
        Self::with_buckets(root, vec![Bucket::new()])
    }

    /// Create a [RoutingTable] with the capacity of its maximum possible number of buckets.
    ///
    /// That is equal to the [NodeId] Size in Bits.
    pub fn with_full_capacity(root: NodeId<ID_SIZE>) -> Result<Self, GroupingError> {
        let mut buckets = Vec::with_capacity(ID_SIZE * 8);
        buckets.push(Bucket::new());
        Self::with_buckets(root, buckets)
    }

    // As soon as 'const where restrictions' are supported
    // this can be converted to a const function.
    fn with_buckets(
        root: NodeId<ID_SIZE>,
        buckets: Vec<Bucket<ID_SIZE, BUCKET_SIZE>>,
    ) -> Result<Self, GroupingError> {
        if ACC < ID_SIZE || ACC == 0 {
            return Err(GroupingError::Invalid {
                id_size: ID_SIZE,
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
        ((ID_SIZE * 8) / ACC) * Self::level_width()
    }

    /// Returns the number of [Bucket]s.
    pub fn num_buckets(&self) -> usize {
        self.buckets.len()
    }

    /// Returns the number of [Contact]s in this [RoutingTable].
    pub fn num_contacts(&self) -> usize {
        self.buckets.iter().flat_map(|bucket| bucket.iter()).count()
    }

    fn get_bucket_index(&self, of: &NodeId<ID_SIZE>) -> usize {
        Self::get_bucket_index_for(of, &self.root, self.num_buckets())
    }

    /// Returns the index of the [Bucket] the id should be in related
    /// to the current state of the [RoutingTable].
    fn get_bucket_index_for(
        of: &NodeId<ID_SIZE>,
        for_root: &NodeId<ID_SIZE>,
        num_buckets: usize,
    ) -> usize {
        let SharedPrefix {
            xor: delta,
            value: prefix_len,
        } = for_root
            .shared_prefix_len(of, ACC)
            .expect("GroupingError after checking");

        // bitindex is now the index of the LSB of the first non-zero digit in delta
        let bit_index = (ID_SIZE * 8).checked_sub((prefix_len + 1) * ACC);
        let bit_index = match bit_index {
            // This is the root key
            None => return num_buckets - 1, // Always at least one bucket present
            Some(bit_index) => bit_index,
        };

        // bitindex is the LSB of the first non-zero digit, so digit must not be zero
        assert_ne!(delta.bits(bit_index, Self::non_zero_acc()), Ok(0));

        assert!(
            bit_index + ACC >= ID_SIZE
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

    /// Returns the Bucket the [NodeId] should be located in based on the
    /// current state of the [RoutingTable].
    fn bucket(&self, of: &NodeId<ID_SIZE>) -> &Bucket<ID_SIZE, BUCKET_SIZE> {
        let index = self.get_bucket_index(of);
        &self.buckets[index]
    }

    /// Returns a mutable reference to the [Bucket] for the given [NodeId].
    fn bucket_mut(&mut self, of: &NodeId<ID_SIZE>) -> &mut Bucket<ID_SIZE, BUCKET_SIZE> {
        let index = self.get_bucket_index(of);
        &mut self.buckets[index]
    }
}

pub struct Iter<'a, const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize> {
    table: &'a FlatRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>,
    index: (usize, usize),
}

impl<'a, const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize>
    Iter<'a, ID_SIZE, BUCKET_SIZE, ACC>
{
    fn new(table: &'a FlatRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>) -> Self {
        Self {
            table,
            index: (0, 0),
        }
    }
}

impl<'a, const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize> Iterator
    for Iter<'a, ID_SIZE, BUCKET_SIZE, ACC>
{
    type Item = &'a Contact<ID_SIZE>;

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

impl<'a, const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<'a, ID_SIZE>
    for FlatRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>
{
    type Iter = Iter<'a, ID_SIZE, BUCKET_SIZE, ACC>;

    fn add(&mut self, contact: Contact<ID_SIZE>) -> Result<(), AddError<ID_SIZE>> {
        let bucket = self.bucket_mut(contact.id());

        match bucket.insert(contact) {
            Err(super::bucket::BucketInsertionError::<ID_SIZE>::Full) => Err(AddError::NotAdded),
            Err(super::bucket::BucketInsertionError::<ID_SIZE>::DuplicateId(id)) => {
                Err(AddError::AlreadyExists(id))
            }
            Ok(_) => Ok(()),
        }
    }

    fn remove(&mut self, id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>> {
        let bucket = self.bucket_mut(id);
        // NOTE: Maybe restructuring the RoutingTable here
        bucket.remove(id)
    }

    fn replace(
        &mut self,
        id: &NodeId<ID_SIZE>,
        with: Contact<ID_SIZE>,
    ) -> Result<(), ReplacementError<ID_SIZE>> {
        let bucket = self.bucket_mut(id);
        bucket.replace(id, with)
    }

    fn contact(&self, id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>> {
        let bucket = self.bucket(id);
        bucket.get(id)
    }

    fn random_id(&self) -> Option<&NodeId<ID_SIZE>> {
        let mut rng = rand::thread_rng();
        let random_contact = rng.gen_range(0..self.num_contacts());
        self.contacts_iter()
            .nth(random_contact)
            .map(|contact| contact.id())
    }

    fn contact_mut(&mut self, id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>> {
        let bucket = self.bucket_mut(id);
        bucket.get_mut(id)
    }

    fn contains(&self, id: &NodeId<ID_SIZE>) -> bool {
        let bucket = self.bucket(id);
        bucket.contains(id)
    }

    fn contacts_iter(&'a self) -> Self::Iter {
        Iter::new(self)
    }

    fn split_bucket(&mut self, id: &NodeId<ID_SIZE>) -> Result<(), BucketSplitError> {
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
}

#[cfg(test)]
mod routing_tests {
    use std::error::Error;

    use crate::domain::{
        AddError, Age, Contact, FlatRoutingTable, NodeId, Path, RoutingTable, StateSeqNr,
    };

    #[test]
    fn test_add() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1>::new(NodeId::<1>::zero())?;

        let contact = Contact::new(
            NodeId::one(),
            Age::from(1),
            Path::empty(),
            StateSeqNr::from(1),
        );

        assert_eq!(table.add(contact), Ok(()));

        Ok(())
    }

    #[test]
    fn test_add_full() -> Result<(), Box<dyn Error>> {
        let mut table = FlatRoutingTable::<1, 1, 1>::new(NodeId::<1>::zero())?;

        table.add(Contact::new(
            NodeId::from([1]),
            Age::from(0),
            Path::empty(),
            StateSeqNr::from(0),
        ))?;

        assert_eq!(
            table.add(Contact::new(
                NodeId::from([2]),
                Age::from(0),
                Path::empty(),
                StateSeqNr::from(0),
            )),
            Err(AddError::NotAdded)
        );

        Ok(())
    }

    #[test]
    fn test_split() -> Result<(), Box<dyn Error>> {
        // Split should move contacts accordingly and bucket_index should change

        let mut table = FlatRoutingTable::<1, 1, 1>::new(NodeId::<1>::zero())?;

        table.add(Contact::new(
            NodeId::one(),
            Age::from(0),
            Path::empty(),
            StateSeqNr::from(0),
        ))?;

        assert_eq!(table.split_bucket(&NodeId::one()), Ok(()));

        assert_eq!(table.get_bucket_index(&NodeId::one()), 1);

        Ok(())
    }
}
