use std::{
    collections::VecDeque,
    fmt::Display,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use futures::{lock::Mutex, StreamExt};
use generic_array::GenericArray;

use rasi::task::spawn_ok;
use xstack::{events, identity::PeerId, EventSource, Switch};

use crate::{Error, Result};

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

    fn len(&self) -> usize {
        self.0.len()
    }
}

/// The distance type outlined in the Kademlia [**paper**]
///
/// [**paper**]: https://doi.org/10.1007/3-540-45748-8_5
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
        let k_index = KBucketKey::from(peer_id)
            .distance(&self.local_key)
            .k_index()
            .expect("Insert local peer's id") as usize;

        if let Some(index) = self.k_index[k_index] {
            let r = self.k_buckets[index].try_insert(peer_id);

            if r.is_none() {
                log::trace!("peer_id={}, insert k-bucket({})", peer_id, k_index);
            }

            r
        } else {
            log::trace!("peer_id={}, insert k-bucket({})", peer_id, k_index,);
            self.k_buckets.push(KBucket::new(peer_id));
            self.k_index[k_index] = Some(self.k_buckets.len() - 1);
            None
        }
    }

    fn closest(&mut self, key: KBucketKey) -> Result<Vec<PeerId>> {
        let k_index = key
            .distance(&self.local_key)
            .k_index()
            .ok_or(Error::Closest)? as usize;

        let mut peers = vec![];

        if let Some(bucket) = self.bucket(k_index) {
            peers = bucket.0.iter().rev().cloned().collect();
        }

        let mut step = 1usize;

        while peers.len() < K {
            if k_index >= step {
                if let Some(bucket) = self.bucket(k_index - step) {
                    if bucket.len() + peers.len() > K {
                        let offset = bucket.len() + peers.len() - K;

                        for peer_id in bucket.0.iter().collect::<Vec<_>>()[offset..].iter().rev() {
                            peers.push(**peer_id);
                        }

                        break;
                    }

                    for peer_id in bucket.0.iter().rev() {
                        peers.push(*peer_id);
                    }
                }
            } else if k_index + step >= self.k_index.len() {
                break;
            }

            if k_index + step < self.k_index.len() {
                if let Some(bucket) = self.bucket(k_index + step) {
                    if bucket.len() + peers.len() > K {
                        let offset = bucket.len() + peers.len() - K;

                        for peer_id in bucket.0.iter().collect::<Vec<_>>()[offset..].iter().rev() {
                            peers.push(**peer_id);
                        }

                        break;
                    }

                    for peer_id in bucket.0.iter().rev() {
                        peers.push(*peer_id);
                    }
                }
            } else if k_index < step {
                break;
            }

            step += 1;
        }

        Ok(peers)
    }

    fn bucket(&self, index: usize) -> Option<&KBucket<K>> {
        self.k_index[index].map(|index| &self.k_buckets[index])
    }
}

/// A rust implementation of the k-bucket data structure outlined in the Kademlia [**paper**]
///
///
/// Unlike the paper description, the only data stored by this implementation is the [`PeerId`],
/// develpers can call [`lookup_peer_info`] function to get the more data associated with the it.
///
/// [**paper**]: https://doi.org/10.1007/3-540-45748-8_5
/// [`lookup_peer_info`]: xstack::Switch::lookup_peer_info
/// [`PeerId`]: xstack::identity::PeerId
#[derive(Clone)]
pub struct KBucketTable<const K: usize = 20> {
    #[allow(unused)]
    /// The switch instance to which this table belongs
    switch: Switch,
    /// the size of this table,
    len: Arc<AtomicUsize>,
    /// The inner raw KBucketTable instance.
    table: Arc<Mutex<RawKBucketTable<K>>>,
}

impl<const K: usize> KBucketTable<K> {
    async fn insert_prv(&self, peer_id: PeerId) -> Option<PeerId> {
        let r = self.table.lock().await.try_insert(peer_id);

        if r.is_none() {
            self.len.fetch_add(1, Ordering::Relaxed);
        }

        r
    }
}

impl<const K: usize> KBucketTable<K> {
    /// Create a `k-bucket` table for provides `switch`
    pub async fn bind(switch: &Switch) -> Self {
        assert!(K > 0, "the k must greater than zero");

        let mut event_connected = EventSource::<events::HandshakeSuccess>::bind_with(
            &switch,
            NonZeroUsize::new(100).unwrap(),
        )
        .await;

        let table = Self {
            len: Default::default(),
            table: Arc::new(Mutex::new(RawKBucketTable {
                local_key: switch.local_id().into(),
                k_buckets: Default::default(),
                k_index: Default::default(),
            })),
            switch: switch.clone(),
        };

        let table_cloned = table.clone();

        // create background update task.
        spawn_ok(async move {
            while let Some((_, peer_id)) = event_connected.next().await {
                table_cloned.insert(peer_id).await;
            }
        });

        table
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
        if let Some(_lru) = self.insert_prv(peer_id.clone()).await {
            // spawn_ok(async move {
            //     // ping the lru to decide what to do.
            //     if let Err(err) = ProtocolStream::ping_with(&this.switch, &lru).await {
            //         log::trace!("ping lru node, {}", err);
            //         this.insert_prv(lru.clone()).await;
            //     } else {
            //         this.insert_prv(peer_id).await;
            //     }
            // });

            // let this = self.clone();

            // // ping the lru to decide what to do.
            // if let Err(err) = ProtocolStream::ping_with(&this.switch, &lru).await {
            //     log::trace!("ping lru node, {}", err);
            //     this.insert_prv(lru).await;
            // } else {
            //     this.insert_prv(peer_id).await;
            // }
        }
    }

    /// Return the closest `K` peers for the input `key`.
    pub async fn closest<Q>(&self, key: Q) -> Result<Vec<PeerId>>
    where
        Q: Into<KBucketKey>,
    {
        let key: KBucketKey = key.into();

        self.table.lock().await.closest(key)
    }
}

#[cfg(test)]
mod tests {

    use std::sync::atomic::AtomicBool;

    use super::{uint::U256, *};

    use quickcheck::*;
    use rasi_mio::{net::register_mio_network, timer::register_mio_timer};
    use xstack::global_switch;
    use xstack_dnsaddr::DnsAddr;
    use xstack_quic::QuicTransport;
    use xstack_tcp::TcpTransport;

    impl Arbitrary for KBucketKey {
        fn arbitrary(_: &mut Gen) -> KBucketKey {
            KBucketKey::from(PeerId::random())
        }
    }

    #[test]
    fn distance_symmetry() {
        fn prop(a: KBucketKey, b: KBucketKey) -> bool {
            a.distance(&b) == b.distance(&a)
        }
        quickcheck(prop as fn(_, _) -> _)
    }

    #[test]
    fn k_distance_0() {
        assert_eq!(KBucketDistance(U256::from(0)).k_index(), None);
        assert_eq!(KBucketDistance(U256::from(1)).k_index(), Some(0));
        assert_eq!(KBucketDistance(U256::from(2)).k_index(), Some(1));
        assert_eq!(KBucketDistance(U256::from(3)).k_index(), Some(1));
        assert_eq!(KBucketDistance(U256::from(4)).k_index(), Some(2));
        assert_eq!(KBucketDistance(U256::from(5)).k_index(), Some(2));
        assert_eq!(KBucketDistance(U256::from(6)).k_index(), Some(2));
        assert_eq!(KBucketDistance(U256::from(7)).k_index(), Some(2));
    }

    #[test]
    fn distance_self() {
        let key = KBucketKey::from(PeerId::random());

        let distance = key.distance(&key);

        assert_eq!(distance.0, U256::from(0));
    }

    async fn init() {
        static INIT: AtomicBool = AtomicBool::new(false);

        if INIT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            // _ = pretty_env_logger::try_init_timed();

            register_mio_network();
            register_mio_timer();

            Switch::new("kad-test")
                .transport(QuicTransport::default())
                .transport(TcpTransport::default())
                .transport(DnsAddr::new().await.unwrap())
                .create()
                .await
                .unwrap()
                .into_global();

            INIT.store(false, Ordering::Release);
        }

        while INIT.load(Ordering::Acquire) {}
    }

    #[futures_test::test]
    async fn test_table() {
        init().await;

        let k_bucket_table = KBucketTable::<20>::bind(global_switch()).await;

        k_bucket_table.insert(PeerId::random()).await;

        assert_eq!(k_bucket_table.len(), 1);

        let closest = k_bucket_table.closest(PeerId::random()).await.unwrap();

        assert_eq!(closest.len(), 1);

        for _ in 1..20 {
            k_bucket_table.insert(PeerId::random()).await;
        }

        assert_eq!(k_bucket_table.len(), 20);

        let closest = k_bucket_table.closest(PeerId::random()).await.unwrap();

        assert_eq!(closest.len(), 20);

        loop {
            k_bucket_table.insert(PeerId::random()).await;

            if k_bucket_table.len() > 20 {
                break;
            }
        }

        let closest = k_bucket_table.closest(PeerId::random()).await.unwrap();

        assert_eq!(closest.len(), 20);
    }
}
