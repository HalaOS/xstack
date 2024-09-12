use std::{collections::HashMap, io::Result, sync::Arc};

use async_trait::async_trait;

use futures::lock::Mutex;
use libp2p_identity::PeerId;
use multiaddr::Multiaddr;
use rand::{
    seq::{IteratorRandom, SliceRandom},
    thread_rng,
};

use crate::{driver_wrapper, P2pConn, Switch};

/// Variant returns by [`Connector::connect`] function
///
/// [`Connector::connect`]: connector_syscall::DriverConnector::connect
pub enum Connected {
    ///  The new established connection, that has not complete handshake.
    New(P2pConn),

    /// The connection with a successful handshake,
    /// generally this connection is handled by a `Connector`.
    Authenticated(P2pConn),
}

/// A `Connector` driver must implement the `Driver-*` traits in this module.
///
pub mod connector_syscall {
    use std::io::Result;

    use async_trait::async_trait;
    use libp2p_identity::PeerId;
    use multiaddr::Multiaddr;

    use crate::{P2pConn, Switch};

    #[async_trait]
    pub trait DriverConnector: Sync + Send {
        /// Connect to peer via `raddr`.
        async fn connect(&self, switch: &Switch, raddrs: &[Multiaddr]) -> Result<P2pConn>;

        /// Connect to peer via `peer_id`.
        async fn connect_to(&self, switch: &Switch, peer_id: &PeerId) -> Result<P2pConn>;

        /// host an incoming [`P2pConn`].
        async fn incoming(&self, conn: P2pConn);

        /// Close a connection by `conn_id` and remove it from thie inner pool.
        async fn close(&self, conn_id: &str);

        /// Returns the inner pool size.
        async fn len(&self) -> usize;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverConnector`](connector_syscall::DriverConnector)"]
    Connector[connector_syscall::DriverConnector]
);

#[derive(Default)]
struct RawConnPool {
    /// maxiumn connections that this pool can contains.
    max_pool_size: usize,
    /// The mapping from peer's listening address to peer_id
    raddrs: HashMap<Multiaddr, PeerId>,
    /// The mapping from peer_id to connection id list.
    peers: HashMap<PeerId, Vec<String>>,
    /// The mapping from connection id to connection.
    conns: HashMap<String, P2pConn>,
}

impl RawConnPool {
    fn new(max_pool_size: usize) -> Self {
        Self {
            max_pool_size,
            ..Default::default()
        }
    }

    fn check_limits(&mut self) {
        if self.conns.len() > self.max_pool_size {
            let mut removed = vec![];

            for (id, conn) in &self.conns {
                if conn.is_closed() || conn.actives() == 0 {
                    removed.push(id.clone());
                }
            }

            for id in removed {
                self.remove(&id);
            }
        }
    }

    /// add a authenticated connection into the pool.
    fn add(&mut self, conn: P2pConn, inbound: bool) {
        self.check_limits();
        let peer_id = conn.public_key().to_peer_id();
        let peer_addr = conn.peer_addr().clone();
        let id = conn.id().to_owned();

        if self.conns.insert(conn.id().to_owned(), conn).is_none() {
            // if this is a outbound connection, create the raddr index.
            if !inbound {
                self.raddrs.insert(peer_addr, peer_id.clone());
            }

            if let Some(ids) = self.peers.get_mut(&peer_id) {
                ids.push(id)
            } else {
                self.peers.insert(peer_id, vec![id]);
            }
        }
    }

    /// Remove a connection from the pool.
    fn remove(&mut self, id: &str) {
        if let Some(mut conn) = self.conns.remove(id) {
            log::trace!("remove conn {}", id);

            assert_eq!(id, conn.id());

            let peer_id = conn.public_key().to_peer_id();

            self.raddrs.remove(conn.peer_addr());

            if let Some(mut ids) = self.peers.remove(&peer_id) {
                if let Some((index, _)) = ids.iter().enumerate().find(|(_, v)| v.as_str() == id) {
                    ids.remove(index);
                }

                if !ids.is_empty() {
                    self.peers.insert(peer_id, ids);
                }
            }

            if !conn.is_closed() {
                _ = conn.close();
            }
        }
    }

    fn by_raddr(&mut self, raddr: &Multiaddr) -> Option<P2pConn> {
        if let Some(peer_id) = self.raddrs.get(raddr) {
            if let Some(ids) = self.peers.get(peer_id) {
                log::trace!("by_raddr: {:?}", ids);
                return ids
                    .iter()
                    .choose(&mut thread_rng())
                    .map(|id| self.conns.get(id).expect("consistency guarantee").clone());
            }
        }

        return None;
    }

    fn by_peer_id(&mut self, peer_id: &PeerId) -> Option<P2pConn> {
        if let Some(ids) = self.peers.get(peer_id) {
            log::trace!("by_peer_id: {:?}", ids);
            return ids
                .iter()
                .choose(&mut thread_rng())
                .map(|id| self.conns.get(id).expect("consistency guarantee").clone());
        }
        return None;
    }

    fn len(&self) -> usize {
        log::trace!(target:"len","conn({}), peers({}), raddrs({})",self.conns.len(),self.peers.len(),self.raddrs.len());
        self.conns.len()
    }
}

/// The default [`Connector`] implementation for `Switch`.
pub struct ConnPool {
    raw: Arc<Mutex<RawConnPool>>,
}

impl Default for ConnPool {
    fn default() -> Self {
        Self::new(20)
    }
}

impl ConnPool {
    /// Create a new `ConnPool` with customise `max_pool_size`.
    pub fn new(max_pool_size: usize) -> Self {
        Self {
            raw: Arc::new(Mutex::new(RawConnPool::new(max_pool_size))),
        }
    }

    async fn connect_raddrs(
        &self,
        switch: &Switch,
        mut raddrs: Vec<&Multiaddr>,
    ) -> Result<P2pConn> {
        raddrs.shuffle(&mut thread_rng());

        let mut last_error = None;

        for raddr in raddrs {
            let conn = match switch.transport_connect(raddr).await {
                Ok(conn) => conn,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };

            self.raw.lock().await.add(conn.clone(), false);

            return Ok(conn);
        }

        Err(last_error.unwrap().into())
    }
}

#[async_trait]
impl connector_syscall::DriverConnector for ConnPool {
    /// Connect to peer via `raddr`.
    async fn connect(&self, switch: &Switch, raddrs: &[Multiaddr]) -> Result<P2pConn> {
        if raddrs.is_empty() {
            return Err(crate::Error::EmptyPeerAddrs.into());
        }

        for raddr in raddrs {
            if let Some(conns) = self.raw.lock().await.by_raddr(raddr) {
                return Ok(conns);
            }
        }

        let raddrs = raddrs.iter().collect::<Vec<_>>();

        self.connect_raddrs(switch, raddrs).await
    }

    /// Connect to peer via `peer_id`.
    async fn connect_to(&self, switch: &Switch, peer_id: &PeerId) -> Result<P2pConn> {
        if let Some(conns) = self.raw.lock().await.by_peer_id(peer_id) {
            return Ok(conns);
        }

        if let Some(peer_info) = switch.lookup_peer_info(peer_id).await? {
            if peer_info.addrs.is_empty() {
                return Err(crate::Error::PeerNotFound.into());
            }

            return self
                .connect_raddrs(switch, peer_info.addrs.iter().collect())
                .await;
        }

        return Err(crate::Error::PeerNotFound.into());
    }

    /// host an incoming [`P2pConn`].
    async fn incoming(&self, conn: P2pConn) {
        self.raw.lock().await.add(conn, true)
    }

    /// Close a connection by `conn_id` and remove it from thie inner pool.
    async fn close(&self, conn_id: &str) {
        self.raw.lock().await.remove(conn_id)
    }

    /// Returns the inner pool size.
    async fn len(&self) -> usize {
        self.raw.lock().await.len()
    }
}
