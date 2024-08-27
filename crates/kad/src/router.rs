use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Debug,
    future::Future,
    num::NonZeroUsize,
    sync::Arc,
    time::Duration,
};

use futures::{channel::mpsc::channel, lock::Mutex, SinkExt, StreamExt};
use identity::PeerId;
use rasi::{task::spawn_ok, timer::TimeoutExt};
use xstack::{book::PeerInfo, multiaddr::Multiaddr, Switch};

use crate::{
    errors::{Error, Result},
    kbucket::{KBucketKey, KBucketTable},
    rpc::KadRpc,
    store::{syscall::DriverKadStore, KadMemoryStore, KadStore},
    Key,
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

/// The context data of recursive routing algorithms.
pub struct Recursively<'a> {
    label: Option<&'a str>,
    /// The search key.
    key: &'a Key,
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
        key: &'a Key,
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

        let (sender, mut receiver) = channel(concurrency);

        let mut pending = 0;

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

                let mut sender = sender.clone();

                spawn_ok(async move {
                    _ = sender.send((peer_id, fut.await)).await;
                });

                pending += 1;

                break;
            }

            log::trace!(
                "{}, candidates={} pending={} concurrency={}",
                self.label.unwrap_or("routing"),
                self.candidates.len(),
                pending,
                concurrency
            );

            while (self.candidates.is_empty() && pending > 0) || pending == concurrency {
                let (peer_id, result) = receiver.next().await.unwrap();

                pending -= 1;

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
            let lhs = Key::from(lhs).distance(self.key);
            let rhs = Key::from(rhs).distance(self.key);

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
            let last_distance = Key::from(last).distance(self.key);
            let distance = Key::from(peer_id).distance(self.key);

            distance < last_distance
        } else {
            true
        }
    }
}

/// The configuration for creating [`Router`] instance.
#[derive(Clone)]
pub struct RouterOptions {
    /// The timeout wait for rpc calls.
    rpc_timeout: Duration,
    /// The maximum kad protocol packet size received from peer.
    max_packet_size: usize,
    /// The kad record store.
    store: Arc<KadStore>,
    /// the maximum concurrency tasks that this route process can starts.
    concurrency: NonZeroUsize,
}

impl Default for RouterOptions {
    fn default() -> Self {
        Self {
            rpc_timeout: Duration::from_secs(10),
            max_packet_size: 1024 * 1024 * 4,
            store: Arc::new(KadMemoryStore::new().into()),
            concurrency: NonZeroUsize::new(20).unwrap(),
        }
    }
}

impl RouterOptions {
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

    /// Set the timeout wait for rpc calls, the default value is `10s`.
    pub fn set_rpc_timeout(mut self, value: Duration) -> Self {
        self.rpc_timeout = value;
        self
    }

    /// Set the maximum packet size received from peer, the default value is `1024 * 1024 * 4` bytes.
    pub fn set_max_packet_size(mut self, value: usize) -> Self {
        self.max_packet_size = value;
        self
    }

    /// Create a new kad router instance with provides boostrap peer `seeds`.
    pub async fn with_seeds<S, E>(self, switch: Switch, seeds: S) -> Result<Router>
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
                .ok_or_else(|| Error::SeedMultAddr(raddr.clone()))?
            {
                xstack::multiaddr::Protocol::P2p(id) => {
                    if let Some(addrs) = peer_addrs.get_mut(&id) {
                        addrs.push(raddr);
                    } else {
                        peer_addrs.insert(id, vec![raddr]);
                    }
                }
                _ => {
                    return Err(Error::SeedMultAddr(raddr.clone()));
                }
            }
        }

        let mut kbucket_table = KBucketTable::<Key, PeerId, 20>::new(Key::from(switch.local_id()));

        for (id, addrs) in peer_addrs {
            kbucket_table.insert(Key::from(&id), id.clone());

            let peer_info = PeerInfo {
                id: id.clone(),
                addrs,
                ..Default::default()
            };

            switch.add_peer(peer_info).await?;
        }

        Ok(Router {
            ops: self,
            switch,
            kbucket: Arc::new(Mutex::new(kbucket_table)),
        })
    }
}

/// A peer and content provider router base on `Kad-DHT` network.
#[allow(unused)]
#[derive(Clone)]
pub struct Router {
    ops: RouterOptions,
    /// The underlying libp2p switch.
    switch: Switch,
    /// a in memory kbucket routing table.
    kbucket: Arc<Mutex<KBucketTable<Key, PeerId, 20>>>,
}

impl Router {
    /// Create a new kad router instance.
    pub fn new() -> RouterOptions {
        RouterOptions::default()
    }

    /// Try get the routing path by [`PeerId`].
    pub async fn find_node(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        let key = Key::from(peer_id);

        let seeds = self
            .kbucket
            .lock()
            .await
            .closest_k(&key)
            .map(|(_, id)| id.clone())
            .collect::<Vec<_>>();

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

        Ok(self.switch.peer_info(peer_id).await?)
    }
}

/// FIND_NODE algorithm implementation.
pub struct FindNode<'a> {
    target: &'a PeerId,
    router: &'a Router,
}

impl<'a> RoutingAlogrithm for FindNode<'a> {
    fn route(
        &self,
        peer_id: &PeerId,
    ) -> impl Future<Output = std::io::Result<Routing>> + Send + 'static {
        let peer_id = peer_id.clone();
        let switch = self.router.switch.clone();
        let target = self.target.clone();
        let max_packet_size = self.router.ops.max_packet_size;
        let timeout = self.router.ops.rpc_timeout;
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
                switch.add_peer(peer_info).await?;
            }

            if finished {
                Ok(Routing::Finished)
            } else {
                Ok(Routing::Closest(candidates))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        str::FromStr,
        sync::atomic::{AtomicBool, Ordering},
    };

    use identity::Keypair;
    use rasi_mio::{net::register_mio_network, timer::register_mio_timer};
    use xstack::{global_switch, transport::ProtocolStream, Switch};
    use xstack_dnsaddr::DnsAddr;
    use xstack_quic::QuicTransport;
    use xstack_tcp::TcpTransport;

    use crate::rpc::{GetProviders, GetValue};

    use super::*;

    async fn init() {
        static INIT: AtomicBool = AtomicBool::new(false);

        if INIT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            _ = pretty_env_logger::try_init_timed();

            register_mio_network();
            register_mio_timer();

            Switch::new("kad-test")
                .transport(QuicTransport::default())
                .transport(TcpTransport)
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
    async fn find_node() {
        init().await;

        let kad = Router::new()
            .with_seeds(
                global_switch().clone(),[
                "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN"
            ])
            .await
            .unwrap();

        let peer_id = PeerId::random();

        let peer_info = kad.find_node(&peer_id).await.unwrap();

        log::info!("find_node: {}, {:?}", peer_id, peer_info);
    }

    #[futures_test::test]
    async fn find_node_1() {
        init().await;

        let kad = Router::new()
            .with_seeds(
                global_switch().clone(),[
               "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN"
            ])
            .await
            .unwrap();

        let peer_id =
            PeerId::from_str("12D3KooWEPZizPnaBj1qvg6BoJ92svF5cyvJergppGZRopajgLZA").unwrap();

        let peer_info = kad.find_node(&peer_id).await.unwrap();

        log::info!("find_node: {}, {:?}", peer_id, peer_info);
    }

    #[futures_test::test]
    async fn put_value() {
        init().await;

        let (stream, _) = ProtocolStream::
            connect(
                 "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
                [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
            )
            .await
            .unwrap();

        let keypair = Keypair::generate_ed25519();

        let id = PeerId::from_public_key(&keypair.public());
        let value = keypair.public().encode_protobuf();

        let mut key = "/pk/".as_bytes().to_vec();

        key.append(&mut id.to_bytes());

        stream
            .kad_put_value(&key, &value, 1024 * 1024)
            .await
            .unwrap();

        let (stream, _) =  ProtocolStream::
            connect(
                 "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
                [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
            )
            .await
            .unwrap();

        let GetValue {
            closer_peers: _,
            value: get_value,
        } = stream.kad_get_value(key, 1024 * 1024).await.unwrap();

        assert_eq!(get_value, Some(value));
    }

    #[futures_test::test]
    async fn add_provider() {
        init().await;

        let (stream, _) =  ProtocolStream::connect(
                "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
                [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
            )
            .await
            .unwrap();

        let id = PeerId::random();

        let peer_info = PeerInfo {
            id: global_switch().local_id().clone(),
            addrs: vec!["/ip4/89.58.16.110/udp/37530/quic-v1".parse().unwrap()],
            ..Default::default()
        };

        stream
            .kad_add_provider(id.to_bytes(), &peer_info)
            .await
            .unwrap();

        let (stream, _) = ProtocolStream::connect(
            "QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
            [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
        )
        .await
        .unwrap();

        let GetProviders {
            closer_peers: _,
            provider_peers,
        } = stream
            .kad_get_providers(id.to_bytes(), 1024 * 1024)
            .await
            .unwrap();

        assert_eq!(provider_peers, vec![peer_info]);
    }
}
