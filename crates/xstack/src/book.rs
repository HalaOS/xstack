use std::{collections::HashMap, fmt::Display, io::Result};

use async_trait::async_trait;
use futures::lock::Mutex;
use identity::PeerId;
use multiaddr::Multiaddr;

use crate::driver_wrapper;

/// The variant of connection status type.
#[derive(Debug, Clone, Copy)]
pub enum ConnectionType {
    /// sender does not have a connection to peer, and no extra information (default)
    NotConnected = 0,
    /// sender has a live connection to peer
    Connected = 1,

    /// sender recently connected to peer
    CanConnect = 2,

    /// sender recently tried to connect to peer repeatedly but failed to connect
    /// ("try" here is loose, but this should signal "made strong effort, failed")
    CannotConnect = 3,
}

/// A type that hold the peer's basic information used by kad protocol.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// The peer's public id.
    pub id: PeerId,
    /// peer listen addresses.
    pub addrs: Vec<Multiaddr>,
    /// The peer's status type.
    pub conn_type: ConnectionType,
}

impl PartialEq for PeerInfo {
    fn eq(&self, other: &Self) -> bool {
        self.id.eq(&other.id) && self.addrs.eq(&other.addrs)
    }
}

impl Default for PeerInfo {
    fn default() -> Self {
        Self {
            id: PeerId::random(),
            addrs: vec![],
            conn_type: ConnectionType::NotConnected,
        }
    }
}

impl Display for PeerInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "id={}, addrs={:?}, conn_type={:?}",
            self.id, self.addrs, self.conn_type
        )
    }
}

/// The `PeerBook` driver must implement the `Driver-*` traits in this module.
pub mod syscall {
    use std::io::Result;

    use async_trait::async_trait;
    use identity::PeerId;
    use multiaddr::Multiaddr;

    use super::{ConnectionType, PeerInfo};

    /// The main entry point of `PeerBook` implementation
    #[async_trait]
    pub trait DriverPeerBook: Sync + Send {
        /// Add new [`PeerInfo`] into the book.
        ///
        /// On success, returns the older version of [`PeerInfo`].
        async fn put(&self, info: PeerInfo) -> Result<Option<PeerInfo>>;

        /// Remove [`PeerInfo`] from the boook by [`peer_id`](PeerId).
        async fn remove(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>>;

        /// Update the connection type of one `peer_id` in the book.
        async fn update_conn_type(
            &self,
            peer_id: &PeerId,
            conn_type: ConnectionType,
        ) -> Result<Option<ConnectionType>>;

        /// Get a [`PeerInfo`] copy of one [`peer_id`](PeerId)
        async fn get(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>>;

        /// Reverse lookup [`PeerId`] by [`raddr`](Multiaddr)
        async fn peer_id_of(&self, raddr: &Multiaddr) -> Result<Option<PeerId>>;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverPeerBook`](syscall::DriverPeerBook)"]
    PeerBook[syscall::DriverPeerBook]
);

#[derive(Default)]
struct RawMemoryPeerBook {
    peer_infos: HashMap<PeerId, PeerInfo>,
    peer_addrs: HashMap<Multiaddr, PeerId>,
}

/// An in memory [`RouteTable`] implementation
#[derive(Default)]
pub struct MemoryPeerBook(Mutex<RawMemoryPeerBook>);

#[async_trait]
impl syscall::DriverPeerBook for MemoryPeerBook {
    async fn put(&self, info: PeerInfo) -> Result<Option<PeerInfo>> {
        log::trace!("MemoryPeerBook, put id={}", info.id);
        let mut raw = self.0.lock().await;
        let id = info.id.clone();

        let raddrs = info.addrs.clone();

        let older = raw.peer_infos.insert(info.id.clone(), info);

        if let Some(old) = &older {
            for raddr in &old.addrs {
                raw.peer_addrs.remove(raddr);
            }
        }

        for raddr in raddrs {
            raw.peer_addrs.insert(raddr, id.clone());
        }

        Ok(older)
    }

    async fn remove(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        let mut raw = self.0.lock().await;

        let older = raw.peer_infos.remove(peer_id);

        if let Some(old) = &older {
            for raddr in &old.addrs {
                raw.peer_addrs.remove(&raddr);
            }
        }

        Ok(older)
    }

    async fn update_conn_type(
        &self,
        peer_id: &PeerId,
        conn_type: ConnectionType,
    ) -> Result<Option<ConnectionType>> {
        if let Some(peer_info) = self.0.lock().await.peer_infos.get_mut(peer_id) {
            let old = peer_info.conn_type;

            peer_info.conn_type = conn_type;

            Ok(Some(old))
        } else {
            Ok(None)
        }
    }

    async fn get(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        Ok(self
            .0
            .lock()
            .await
            .peer_infos
            .get(&peer_id)
            .map(|info| info.clone()))
    }

    async fn peer_id_of(&self, raddr: &Multiaddr) -> Result<Option<PeerId>> {
        Ok(self
            .0
            .lock()
            .await
            .peer_addrs
            .get(raddr)
            .map(|id| id.clone()))
    }
}
