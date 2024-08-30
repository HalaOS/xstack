use std::{
    collections::VecDeque,
    fmt::Display,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use futures::lock::Mutex;
use generic_array::GenericArray;
use libp2p_identity::PeerId;

use rasi::task::spawn_ok;
use xstack::{ProtocolStream, Switch};

mod uint {
    use uint::construct_uint;
    construct_uint! {
        pub(super) struct U256(4);
    }
}

#[derive(Default)]
struct KBucket<const K: usize>(VecDeque<PeerId>);

impl<const K: usize> KBucket<K> {
    fn new(peer_id: PeerId) -> Self {
        let mut q = VecDeque::new();
        q.push_back(peer_id);
        Self(q)
    }
    fn try_insert(&mut self, peer_id: PeerId) -> Option<PeerId> {
        // already existing, move to the tail
        if let Some(index) =
            self.0
                .iter()
                .enumerate()
                .find_map(|(index, item)| if *item == peer_id { Some(index) } else { None })
        {
            self.0.remove(index);
            self.0.push_back(peer_id);
            return None;
        }

        if self.0.len() == K {
            let lru = self.0.pop_front();
            return lru;
        }

        self.0.push_back(peer_id);

        return None;
    }
}

/// The distance type outlined in the Kademlia [**paper**]
///
/// [**paper**]: https://doi.org/10.1007/3-540-45748-8_5
pub struct KBucketDistance(uint::U256);

impl KBucketDistance {
    /// Returns the integer part of the base 2 logarithm of the [`KBucketDistance`].
    ///
    /// Returns `None` if the distance is zero.
    pub fn k_index(&self) -> Option<u32> {
        (256 - self.0.leading_zeros()).checked_sub(1)
    }
}

impl Display for KBucketDistance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "k_distance({:#066x})", self.0)
    }
}

/// The key type of [`KBucketTable`]
pub struct KBucketKey(uint::U256);

impl KBucketKey {
    pub fn distance(&self, rhs: &KBucketKey) -> KBucketDistance {
        KBucketDistance(self.0 ^ rhs.0)
    }
}

impl From<Vec<u8>> for KBucketKey {
    fn from(value: Vec<u8>) -> Self {
        Self::from(value.as_slice())
    }
}

impl From<&Vec<u8>> for KBucketKey {
    fn from(value: &Vec<u8>) -> Self {
        Self::from(value.as_slice())
    }
}

impl From<&[u8]> for KBucketKey {
    fn from(value: &[u8]) -> Self {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();

        hasher.update(value);

        let buf: [u8; 32] = hasher.finalize().into();

        Self(buf.into())
    }
}

impl From<PeerId> for KBucketKey {
    fn from(value: PeerId) -> Self {
        Self::from(&value)
    }
}

impl From<&PeerId> for KBucketKey {
    fn from(value: &PeerId) -> Self {
        Self::from(value.to_bytes().as_slice())
    }
}

#[allow(unused)]
struct RawKBucketTable<const K: usize> {
    local_key: KBucketKey,
    /// k-bucket list with lazy initialization.
    k_buckets: Vec<KBucket<K>>,
    /// index of k_bucket in `k_buckets`
    k_index: GenericArray<Option<usize>, generic_array::typenum::U256>,
}

impl<const K: usize> RawKBucketTable<K> {
    /// On success, returns None.
    fn try_insert(&mut self, peer_id: PeerId) -> Option<PeerId> {
        let index = KBucketKey::from(peer_id)
            .distance(&self.local_key)
            .k_index()
            .expect("Insert local peer's id") as usize;

        if let Some(index) = self.k_index[index] {
            return self.k_buckets[index].try_insert(peer_id);
        } else {
            self.k_buckets.push(KBucket::new(peer_id));
            None
        }
    }

    fn closest(&mut self, _key: KBucketKey) -> Vec<PeerId> {
        todo!()
    }
}

/// A rust implementation of the k-bucket data structure outlined in the Kademlia [**paper**]
///
///
/// Unlike the paper description, the only data stored by this implementation is the [`PeerId`],
/// develpers can call [`peer_info`] function to get the more data associated with the it.
///
/// [**paper**]: https://doi.org/10.1007/3-540-45748-8_5
/// [`peer_info`]: xstack::Switch::peer_info
/// [`PeerId`]: xstack::identity::PeerId
#[derive(Clone)]
pub struct KBucketTable<const K: usize> {
    /// The switch instance to which this table belongs
    switch: Switch,
    /// the size of this table,
    len: Arc<AtomicUsize>,
    /// The inner raw KBucketTable instance.
    table: Arc<Mutex<RawKBucketTable<K>>>,
}

impl<const K: usize> KBucketTable<K> {
    async fn insert_prv(&self, peer_id: PeerId) -> Option<PeerId> {
        self.table.lock().await.try_insert(peer_id)
    }
}

impl<const K: usize> KBucketTable<K> {
    /// Create a `k-bucket` table from `switch`
    pub fn new(switch: Switch) -> Self {
        assert!(K > 0, "the k must greater than zero");
        Self {
            len: Default::default(),
            table: Arc::new(Mutex::new(RawKBucketTable {
                local_key: switch.local_id().into(),
                k_buckets: Default::default(),
                k_index: Default::default(),
            })),
            switch,
        }
    }
    /// Returns the const k value of the k-bucket.
    pub fn k_const(&self) -> usize {
        K
    }

    /// Get the size of the *routing table*.
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    /// Try insert a peer into the *routing table*,
    /// If you need to understand why `PeerId` is probabilistically inserted into the *routing table*, see the  Kademlia [**paper**]
    ///
    ///
    /// [**paper**]: https://doi.org/10.1007/3-540-45748-8_5
    pub async fn insert(&self, peer_id: PeerId) {
        if let Some(lru) = self.insert_prv(peer_id.clone()).await {
            let this = self.clone();

            spawn_ok(async move {
                // ping the lru to decide what to do.
                if let Err(err) = ProtocolStream::ping(&this.switch, &lru).await {
                    log::trace!("ping lru node, {}", err);
                    this.insert_prv(lru.clone()).await;
                } else {
                    this.insert_prv(peer_id).await;
                }
            });
        }
    }

    /// Return the closest `K` peers for the input `key`.
    pub async fn closest<Q>(&self, key: Q) -> Vec<PeerId>
    where
        Q: Into<KBucketKey>,
    {
        let key: KBucketKey = key.into();

        self.table.lock().await.closest(key)
    }
}
