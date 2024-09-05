use std::{collections::HashMap, io::Result, sync::Arc};

use async_trait::async_trait;

use futures::lock::Mutex;
use libp2p_identity::PeerId;
use multiaddr::{Multiaddr, Protocol};
use rand::{seq::SliceRandom, thread_rng};

use crate::{driver_wrapper, P2pConn, Switch, Transport};

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
    use multiaddr::Multiaddr;

    use crate::{P2pConn, Switch, Transport};

    use super::Connected;

    #[async_trait]
    pub trait DriverConnector: Sync + Send {
        /// Connect to peer via `raddr`.
        async fn connect(
            &self,
            switch: &Switch,
            transport: &Transport,
            raddr: &Multiaddr,
        ) -> Result<Connected>;

        /// Try reuse connection from the cache pool.
        async fn reuse_connect(&self, raddr: &Multiaddr) -> Option<P2pConn>;

        /// Put a connection with a successful handshake back into the connector pool.
        async fn authenticated(&self, conn: P2pConn, inbound: bool);
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverConnector`](connector_syscall::DriverConnector)"]
    Connector[connector_syscall::DriverConnector]
);

#[derive(Default)]
struct RawConnPool {
    /// The mapping from peer's listening address to peer_id
    raddrs: HashMap<Multiaddr, PeerId>,
    /// The mapping from peer_id to connection id list.
    peers: HashMap<PeerId, Vec<String>>,
    /// The mapping from connection id to connection.
    conns: HashMap<String, P2pConn>,
}

impl RawConnPool {
    /// add a authenticated connection into the pool.
    fn add(&mut self, conn: P2pConn, inbound: bool) {
        let peer_id = conn.public_key().to_peer_id();

        // if this is a outbound connection, create the raddr index.
        if !inbound {
            self.raddrs
                .insert(conn.peer_addr().clone(), peer_id.clone());
        }

        if let Some(ids) = self.peers.get_mut(&peer_id) {
            ids.push(conn.id().to_owned())
        } else {
            self.peers.insert(peer_id, vec![conn.id().to_owned()]);
        }

        self.conns.insert(conn.id().to_owned(), conn);
    }

    /// Remove a connection from the pool.
    fn remove(&mut self, id: &str) {
        if let Some(conn) = self.conns.remove(id) {
            let peer_id = conn.public_key().to_peer_id();

            self.raddrs.remove(conn.peer_addr());

            if let Some(ids) = self.peers.get_mut(&peer_id) {
                let (index, _) = ids
                    .iter()
                    .enumerate()
                    .find(|(_, v)| v.as_str() == conn.id())
                    .expect("consistency guarantee");

                ids.remove(index);
            }
        }
    }

    fn by_raddr(&mut self, raddr: &Multiaddr) -> Option<Vec<P2pConn>> {
        if let Some(peer_id) = self.raddrs.get(raddr) {
            if let Some(ids) = self.peers.get(peer_id) {
                log::trace!("{:?}", ids);
                return Some(
                    ids.iter()
                        .map(|id| self.conns.get(id).expect("consistency guarantee").clone())
                        .collect(),
                );
            }
        }

        return None;
    }

    fn by_peer_id(&mut self, peer_id: &PeerId) -> Option<Vec<P2pConn>> {
        if let Some(ids) = self.peers.get(peer_id) {
            log::trace!("{:?}", ids);
            return Some(
                ids.iter()
                    .map(|id| self.conns.get(id).expect("consistency guarantee").clone())
                    .collect(),
            );
        }
        return None;
    }
}

/// The default [`Connector`] implementation for `Switch`.
#[derive(Default)]
pub struct ConnPool {
    raw: Arc<Mutex<RawConnPool>>,
}

#[async_trait]
impl connector_syscall::DriverConnector for ConnPool {
    async fn connect(
        &self,
        switch: &Switch,
        transport: &Transport,
        raddr: &Multiaddr,
    ) -> Result<Connected> {
        // first, try get connection in the pool.
        if let Some(conn) = self.reuse_connect(raddr).await {
            return Ok(Connected::Authenticated(conn));
        }

        log::trace!("connect to {}, new", raddr);
        Ok(Connected::New(transport.connect(switch, raddr).await?))
    }

    async fn reuse_connect(&self, raddr: &Multiaddr) -> Option<P2pConn> {
        let mut raw = self.raw.lock().await;

        let conns = if let Some(Protocol::P2p(peer_id)) = raddr.clone().pop() {
            raw.by_peer_id(&peer_id)
        } else {
            raw.by_raddr(raddr)
        };

        if let Some(mut conns) = conns {
            // shuffle the result.
            conns.shuffle(&mut thread_rng());

            for conn in conns {
                if conn.is_closed() {
                    // remove closed connection.
                    raw.remove(conn.id());
                    continue;
                }

                log::trace!("connect to {}, reused {}", raddr, conn.peer_addr());
                return Some(conn);
            }
        }

        None
    }

    /// Put a connection with a successful handshake back into the connector pool.
    async fn authenticated(&self, conn: P2pConn, inbound: bool) {
        self.raw.lock().await.add(conn, inbound);
    }
}
