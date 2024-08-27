use std::collections::HashMap;

use identity::PeerId;

use crate::{multiaddr::ToSockAddr, transport::TransportConnection};

/// An in-memory connection pool.
#[derive(Default)]
pub(super) struct ConnPool {
    max_pool_size: usize,
    /// mapping id => connection.
    conns: HashMap<String, TransportConnection>,
    /// mapping peer_id to conn id.
    peers: HashMap<PeerId, Vec<String>>,
}

impl ConnPool {
    pub(super) fn new(max_pool_size: usize) -> Self {
        Self {
            max_pool_size,
            ..Default::default()
        }
    }
    pub(super) fn conn_pool_gc(&mut self) {
        log::trace!("conn_pool_gc, size={}", self.conns.len());

        if self.conns.len() < self.max_pool_size {
            return;
        }

        let mut removed = vec![];

        for (_, conn) in &self.conns {
            if conn.actives() == 0 {
                removed.push(conn.clone());
            }
        }

        log::trace!(
            "conn_pool_gc, size={}, removed={}",
            self.conns.len(),
            removed.len()
        );

        for conn in removed {
            self.remove(&conn);
        }
    }

    /// Put a new connecton instance into the pool, and update indexers.
    pub(super) fn put(&mut self, conn: TransportConnection) {
        self.conn_pool_gc();

        let peer_id = conn.public_key().to_peer_id();

        let raddr = conn
            .peer_addr()
            .to_sockaddr()
            .expect("Invalid transport peer_addr.");

        let id = conn.id().to_owned();

        // consistency test.
        if let Some(conn) = self.conns.get(&id) {
            let o_peer_id = conn.public_key().to_peer_id();

            let o_raddr = conn
                .peer_addr()
                .to_sockaddr()
                .expect("Invalid transport peer_addr.");

            assert_eq!(peer_id, o_peer_id, "consistency guarantee");
            assert_eq!(o_raddr, raddr, "consistency guarantee");

            return;
        }

        log::info!(target: "Switch","add new conn, id={}, raddr={}, peer={}",id , raddr, peer_id);

        self.conns.insert(id.to_owned(), conn);

        if let Some(conn_ids) = self.peers.get_mut(&peer_id) {
            conn_ids.push(id);
        } else {
            self.peers.insert(peer_id, vec![id]);
        }
    }

    pub(super) fn get(&self, peer_id: &PeerId) -> Option<Vec<TransportConnection>> {
        if let Some(conn_ids) = self.peers.get(&peer_id) {
            Some(
                conn_ids
                    .iter()
                    .map(|id| self.conns.get(id).expect("consistency guarantee").clone())
                    .collect(),
            )
        } else {
            None
        }
    }

    pub(super) fn remove(&mut self, conn: &TransportConnection) {
        let peer_id = conn.public_key().to_peer_id();

        let raddr = conn
            .peer_addr()
            .to_sockaddr()
            .expect("Invalid transport peer_addr.");

        let id = conn.id().to_owned();

        if let Some(conn) = self.conns.remove(&id) {
            let o_peer_id = conn.public_key().to_peer_id();

            let o_raddr = conn
                .peer_addr()
                .to_sockaddr()
                .expect("Invalid transport peer_addr.");

            assert_eq!(peer_id, o_peer_id, "consistency guarantee");
            assert_eq!(o_raddr, raddr, "consistency guarantee");
        }

        log::info!(target: "Switch","remove conn, id={}, raddr={}, peer={}",id , raddr, peer_id);

        if let Some(conn_ids) = self.peers.get_mut(&peer_id) {
            if let Some((index, _)) = conn_ids.iter().enumerate().find(|(_, v)| **v == id) {
                conn_ids.remove(index);
            }
        }
    }
}
