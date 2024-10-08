use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Debug,
    future::Future,
    num::NonZeroUsize,
    sync::Arc,
};

use futures::StreamExt;
use futures_map::FuturesUnorderedMap;
use rasi::timer::TimeoutExt;
use xstack::{global_switch, identity::PeerId, multiaddr::Multiaddr, PeerInfo, Switch};

use crate::{
    syscall::DriverKadStore, Error, KBucketKey, KBucketTable, KadMemoryStore, KadStore,
    KademliaRpc, Result,
};

/// protocol name of libp2p kad.
pub const PROTOCOL_IPFS_KAD: &str = "/ipfs/kad/1.0.0";
pub const PROTOCOL_IPFS_LAN_KAD: &str = "/ipfs/lan/kad/1.0.0";

/// The varaint returns by call `routing` fn rpc on `peers`
pub enum Routing {
    /// Need to continue recursive routing `closest` peers.
    Closest(Vec<PeerId>),
    /// No need to continue recursive routing
    Finished,
}

/// A recursive routing algorithm must implement this trait.
pub trait RoutingAlogrithm {
    /// Create a future to execute the routing algorithm.
    ///
    /// - `peer_id`, The peer's id on that the routing algorithm executes.
    fn route(
        &self,
        peer_id: &PeerId,
    ) -> impl Future<Output = std::io::Result<Routing>> + Send + 'static;
}

/// The context data for recursive routing algorithms.
pub struct Recursively<'a> {
    label: Option<&'a str>,
    /// The search key.
    key: &'a KBucketKey,
    /// The result k closest nodes.
    closest_k: Vec<PeerId>,
    /// The track set of peers we've already queried.
    queried: HashSet<PeerId>,
    /// The set of next query candidates
    candidates: VecDeque<PeerId>,
    /// the maximum concurrency tasks that this route process can starts.
    concurrency: usize,
    /// The replication parameter.
    const_k: usize,
}

impl<'a> Recursively<'a> {
    /// Create a new recursive routing context data with provides `seeds`.
    /// # parameters
    ///
    /// - `concurrency`, the maximum concurrency tasks that this route process can starts.
    pub fn new(
        label: Option<&'a str>,
        key: &'a KBucketKey,
        const_k: NonZeroUsize,
        concurrency: NonZeroUsize,
        seeds: Vec<PeerId>,
    ) -> Self {
        Self {
            const_k: const_k.into(),
            label,
            key,
            candidates: seeds.into(),
            closest_k: Default::default(),
            queried: Default::default(),
            concurrency: concurrency.into(),
        }
    }

    /// Invoke the recursive routing algorithm.
    pub async fn route<R>(mut self, alg: &R) -> Result<Vec<PeerId>>
    where
        R: RoutingAlogrithm,
    {
        let concurrency = self.concurrency.into();

        let mut unorderd = FuturesUnorderedMap::new();

        while let Some(peer_id) = self.candidates.pop_front() {
            loop {
                if !self.queried.insert(peer_id) {
                    log::trace!(
                        "{}, queried peer_id={}",
                        self.label.unwrap_or("routing"),
                        peer_id
                    );

                    break;
                }

                if !self.is_closer(&peer_id) {
                    log::trace!(
                        "{}, farther peer_id={}",
                        self.label.unwrap_or("routing"),
                        peer_id
                    );

                    break;
                }

                log::debug!(
                    "{}, query peer_id={}",
                    self.label.unwrap_or("routing"),
                    peer_id
                );

                let fut = alg.route(&peer_id);

                unorderd.insert(peer_id, fut);

                break;
            }

            log::trace!(
                "{}, candidates={} pending={} concurrency={}",
                self.label.unwrap_or("routing"),
                self.candidates.len(),
                unorderd.len(),
                concurrency
            );

            while (self.candidates.is_empty() && !unorderd.is_empty())
                || unorderd.len() == concurrency
            {
                let (peer_id, result) = unorderd.next().await.unwrap();

                match result {
                    Ok(Routing::Closest(peers)) => {
                        log::trace!(
                            "{}, query peer_id={}, rx closest={}",
                            self.label.unwrap_or("routing"),
                            peer_id,
                            peers.len()
                        );

                        for peer_id in peers {
                            if self.queried.contains(&peer_id) {
                                continue;
                            }

                            if !self.is_closer(&peer_id) {
                                continue;
                            }

                            self.candidates.push_back(peer_id);
                        }

                        self.add_closest_k(peer_id);
                    }
                    Ok(Routing::Finished) => {
                        log::trace!(
                            "{}, query peer_id={}, done",
                            self.label.unwrap_or("routing"),
                            peer_id,
                        );

                        self.add_closest_k(peer_id);

                        return Ok(self.closest_k);
                    }
                    Err(err) => {
                        log::error!(
                            "{}, query peer_id={}, err={}",
                            self.label.unwrap_or("routing"),
                            peer_id,
                            err
                        );
                    }
                }
            }
        }

        return Ok(self.closest_k);
    }

    fn add_closest_k(&mut self, peer_id: PeerId) {
        self.closest_k.push(peer_id);

        self.closest_k.sort_by(|lhs, rhs| {
            let lhs = KBucketKey::from(lhs).distance(self.key);
            let rhs = KBucketKey::from(rhs).distance(self.key);

            lhs.cmp(&rhs)
        });

        let const_k = self.const_k.into();

        if self.closest_k.len() > const_k {
            self.closest_k.truncate(const_k);
        }

        log::trace!(
            "{}, update closest_k={}",
            self.label.unwrap_or("routing"),
            self.closest_k.len(),
        );
    }

    fn is_closer(&self, peer_id: &PeerId) -> bool {
        if self.closest_k.len() < self.const_k {
            return true;
        }

        if let Some(last) = self.closest_k.last() {
            let last_distance = KBucketKey::from(last).distance(self.key);
            let distance = KBucketKey::from(peer_id).distance(self.key);

            distance < last_distance
        } else {
            true
        }
    }
}

/// The configuration for creating [`KademliaRouter`] instance.
#[derive(Clone)]
pub struct KademliaOptions {
    switch: Switch,
    /// The kad record store.
    store: Arc<KadStore>,
    /// the maximum concurrency tasks that this route process can starts.
    concurrency: NonZeroUsize,
}

impl KademliaOptions {
    fn new(switch: Switch) -> Self {
        Self {
            switch,
            store: Arc::new(KadMemoryStore::new().into()),
            concurrency: NonZeroUsize::new(20).unwrap(),
        }
    }
}

impl KademliaOptions {
    /// Set the maximum concurrency tasks that this route process can starts,
    /// the default value is a `3`.
    pub fn set_concurrency(mut self, value: NonZeroUsize) -> Self {
        self.concurrency = value;
        self
    }

    /// Set the [`KadStore`] instance used by the router,
    /// the default value is a instance of [`KadMemoryStore`].
    pub fn set_store<S>(mut self, value: S) -> Self
    where
        S: DriverKadStore + 'static,
    {
        self.store = Arc::new(value.into());
        self
    }

    /// Create a new kad router instance with provides boostrap peer `seeds`.
    pub async fn with_seeds<S, E>(self, seeds: S) -> Result<KademliaRouter>
    where
        S: IntoIterator,
        S::Item: TryInto<Multiaddr, Error = E>,
        E: Debug,
    {
        let mut peer_addrs = HashMap::<PeerId, Vec<Multiaddr>>::new();

        for raddr in seeds.into_iter() {
            let raddr = raddr
                .try_into()
                .map_err(|err| Error::Other(format!("{:?}", err)))?;

            match raddr
                .clone()
                .pop()
                .ok_or_else(|| Error::WithoutP2p(raddr.clone()))?
            {
                xstack::multiaddr::Protocol::P2p(id) => {
                    if let Some(addrs) = peer_addrs.get_mut(&id) {
                        addrs.push(raddr);
                    } else {
                        peer_addrs.insert(id, vec![raddr]);
                    }
                }
                _ => {
                    return Err(Error::WithoutP2p(raddr.clone()));
                }
            }
        }

        let k_bucket_table = KBucketTable::bind(&self.switch).await;

        for (id, addrs) in peer_addrs {
            k_bucket_table.insert(id).await;

            let peer_info = PeerInfo {
                id: id.clone(),
                addrs,
                ..Default::default()
            };

            self.switch.insert_peer_info(peer_info).await?;
        }

        Ok(KademliaRouter {
            ops: self,
            k_bucket_table,
        })
    }
}

/// A network node that implement the [***ibp2p Kademlia DHT specification***]
///
/// [***ibp2p Kademlia DHT specification***]: https://github.com/libp2p/specs/tree/master/kad-dht
#[derive(Clone)]
pub struct KademliaRouter {
    ops: KademliaOptions,
    k_bucket_table: KBucketTable<20>,
}

impl KademliaRouter {
    /// Create a new kad router instance.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub fn new() -> KademliaOptions {
        KademliaOptions::new(global_switch().clone())
    }

    /// Use provides `Switch` to create a new `KademliaRouter` instance.
    pub fn with(switch: &Switch) -> KademliaOptions {
        KademliaOptions::new(switch.clone())
    }

    /// Try get the routing path by [`PeerId`].
    pub async fn find_node(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        let key = KBucketKey::from(peer_id);

        let seeds = self.k_bucket_table.closest(key).await?;

        let find_node = FindNode {
            target: peer_id,
            router: self,
        };

        Recursively::new(
            Some("FIND_NODE"),
            &key,
            NonZeroUsize::new(20).unwrap(),
            self.ops.concurrency,
            seeds,
        )
        .route(&find_node)
        .await?;

        Ok(self.ops.switch.lookup_peer_info(peer_id).await?)
    }

    /// Returns the routing_table length.
    pub fn len(&self) -> usize {
        self.k_bucket_table.len()
    }
}

/// FIND_NODE algorithm implementation.
pub struct FindNode<'a> {
    target: &'a PeerId,
    router: &'a KademliaRouter,
}

impl<'a> RoutingAlogrithm for FindNode<'a> {
    fn route(
        &self,
        peer_id: &PeerId,
    ) -> impl Future<Output = std::io::Result<Routing>> + Send + 'static {
        let peer_id = peer_id.clone();
        let switch = self.router.ops.switch.clone();
        let target = self.target.clone();
        let max_packet_size = switch.max_packet_size;
        let timeout = switch.timeout;
        // let kbucket = self.router.kbucket.clone();

        async move {
            if peer_id == target {
                return Ok(Routing::Finished);
            }

            let (stream, _) = switch
                .connect(peer_id, [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD])
                .timeout(timeout)
                .await
                .ok_or(Error::Timeout)??;

            let closest_k = stream
                .kad_find_node(target.to_bytes(), max_packet_size)
                .timeout(timeout)
                .await
                .ok_or(Error::Timeout)??;

            let mut candidates = vec![];

            let mut finished = false;

            for peer_info in closest_k {
                if peer_info.id == target {
                    finished = true;
                }

                candidates.push(peer_info.id);
                switch.insert_peer_info(peer_info).await?;
            }

            if finished {
                Ok(Routing::Finished)
            } else {
                Ok(Routing::Closest(candidates))
            }
        }
    }
}
