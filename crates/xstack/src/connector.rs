use std::{collections::HashMap, io::Result, sync::Arc};

use async_trait::async_trait;

use futures::lock::Mutex;
use libp2p_identity::PeerId;
use multiaddr::{Multiaddr, Protocol};
use rand::{seq::SliceRandom, thread_rng};

use crate::{driver_wrapper, Switch, Transport, TransportConnection};

/// Variant returns by [`Connector::connect`] function
///
/// [`Connector::connect`]: connector_syscall::DriverConnector::connect
pub enum Connected {
    ///  The new established connection, that has not complete handshake.
    New(TransportConnection),

    /// The connection with a successful handshake,
    /// generally this connection is handled by a `Connector`.
    Authenticated(TransportConnection),
}

/// A `Connector` driver must implement the `Driver-*` traits in this module.
///
pub mod connector_syscall {
    use std::io::Result;

    use async_trait::async_trait;
    use multiaddr::Multiaddr;

    use crate::{Switch, Transport, TransportConnection};

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
        async fn reuse_connect(&self, raddr: &Multiaddr) -> Option<TransportConnection>;

        /// Put a connection with a successful handshake back into the connector pool.
        async fn authenticated(&self, conn: TransportConnection, inbound: bool);
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
    conns: HashMap<String, TransportConnection>,
}

impl RawConnPool {
    /// add a authenticated connection into the pool.
    fn add(&mut self, conn: TransportConnection, inbound: bool) {
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
                if let Some((index, _)) = ids
                    .iter()
                    .enumerate()
                    .find(|(_, v)| v.as_str() == conn.id())
                {
                    ids.remove(index);
                }
            }
        }
    }

    fn by_raddr(&mut self, raddr: &Multiaddr) -> Option<Vec<TransportConnection>> {
        if let Some(peer_id) = self.raddrs.get(raddr) {
            if let Some(ids) = self.peers.get(peer_id) {
                return Some(
                    ids.iter()
                        .map(|id| self.conns.get(id).expect("consistency guarantee").clone())
                        .collect(),
                );
            }
        }

        return None;
    }

    fn by_peer_id(&mut self, peer_id: &PeerId) -> Option<Vec<TransportConnection>> {
        if let Some(ids) = self.peers.get(peer_id) {
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
            return Ok(Connected::New(conn));
        }

        Ok(Connected::New(transport.connect(switch, raddr).await?))
    }

    async fn reuse_connect(&self, raddr: &Multiaddr) -> Option<TransportConnection> {
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

                return Some(conn);
            }
        }

        None
    }

    /// Put a connection with a successful handshake back into the connector pool.
    async fn authenticated(&self, conn: TransportConnection, inbound: bool) {
        self.raw.lock().await.add(conn, inbound);
    }
}
