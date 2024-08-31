use std::{collections::HashMap, fmt::Display, io::Result, time::SystemTime};

use async_trait::async_trait;
use futures::lock::Mutex;
use libp2p_identity::PeerId;
use multiaddr::Multiaddr;

use crate::driver_wrapper;
/// A `PeerInfo` combines a Peer ID with a set of multiaddrs that the peer is listening on.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// The peer's public id.
    pub id: PeerId,
    /// peer listening addresses.
    pub addrs: Vec<Multiaddr>,
    /// The list of peer support protocols.
    pub protos: Vec<String>,
    /// The timestamp of when the peer latest connected.
    /// `Switch` sets this field after a handshake via the ‘/ipfs/id/1.0.0’ protocol.
    pub appear: Option<SystemTime>,
    /// The timestamp of when the peer last disconnected.
    /// this field is set after disconnecting whether or not the handshake is completed.
    pub disappear: Option<SystemTime>,
}

impl Default for PeerInfo {
    fn default() -> Self {
        Self {
            id: PeerId::random(),
            addrs: Default::default(),
            appear: Default::default(),
            disappear: Default::default(),
            protos: Default::default(),
        }
    }
}

impl PartialEq for PeerInfo {
    fn eq(&self, other: &Self) -> bool {
        self.id.eq(&other.id) && self.addrs.eq(&other.addrs)
    }
}

impl Display for PeerInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "id={}, addrs={:?}, appear={:?}, disappear={:?}",
            self.id, self.addrs, self.appear, self.disappear
        )
    }
}

/// The `PeerBook` driver must implement the `Driver-*` traits in this module.
///
/// A `PeerBook` is a type of data struture that the `Switch` used it to hold peers's networking informations.
///
///
/// This is part of **XSTACK** modularity, developers can call [`peer_book`](crate::SwitchBuilder::peer_book)
/// to set customize implementation; the default implementation used by `Switch` is [`MemoryPeerBook`].
/// ```no_run
/// use xstack::Switch;
///
/// # async fn boostrap() {
/// Switch::new("test")
///       // .peer_book(DatabasePeerBook)
///       .transport_bind(["/ip4/127.0.0.1/tcp/0"])
///       .create()
///       .await
///       .unwrap()
///       // register to global context.
///       .into_global();
/// # }
///```
/// ## Reverse-lookup peer-id
/// The ability to reverse-lookup [`PeerId`] by address is important for `Swich` to
/// reuse existing transport layer connections as much as possible,
/// and is provided by the function [`listen_on`](peerbook_syscall::DriverPeerBook::listen_on).
///
/// ## Extensibility
///
/// Also, if you want to provide features, such as: node information persistence, networking analysis,etc.
/// rewriting `PeerBook` is a good choice!
///
/// In the future, the **XSTACK** will provide more usable information for networking analysis programming
/// by extending [`PeerInfo`] struture.
pub mod peerbook_syscall {
    use std::{io::Result, time::SystemTime};

    use async_trait::async_trait;
    use libp2p_identity::PeerId;
    use multiaddr::Multiaddr;

    use super::PeerInfo;

    /// The customize `PeerBook` must implement this trait.
    ///
    /// The stabilization of async functions in traits in Rust 1.75 did not include support for
    /// using traits containing async functions as dyn Trait, so we use the [**async_trait**](https://docs.rs/async-trait/)
    /// crate to define this trait, to know how to implement the async trait, visit its documentation.
    #[async_trait]
    pub trait DriverPeerBook: Sync + Send {
        /// Insert a new peer informations.
        ///
        /// Returns an older version of the [`PeerInfo`] if existing.
        async fn insert(&self, peer_info: PeerInfo) -> Result<Option<PeerInfo>>;

        /// Remove one peer's [`PeerInfo`] from the book indicated by [`peer_id`](PeerId).
        async fn remove(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>>;

        /// Call this function when the peer is connected.
        /// when the peer is non-existent in the book, returns [`Error::PeerNotFound`](crate::Error::PeerNotFound)
        async fn appear(&self, peer_id: &PeerId, timestamp: SystemTime) -> Result<()>;

        /// Call this function when the peer is disconnected.
        /// when the peer is non-existent in the book, returns [`Error::PeerNotFound`](crate::Error::PeerNotFound)
        async fn disappear(&self, peer_id: &PeerId, timestamp: SystemTime) -> Result<()>;

        /// Fetch the [`PeerInfo`] indicated by [`peer_id`](PeerId).
        async fn get(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>>;

        /// Reverse lookup [`PeerId`] by [`raddr`](Multiaddr)
        async fn listen_on(&self, raddr: &Multiaddr) -> Result<Option<PeerId>>;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverPeerBook`](peerbook_syscall::DriverPeerBook)"]
    PeerBook[peerbook_syscall::DriverPeerBook]
);

#[derive(Default)]
struct RawMemoryPeerBook {
    peer_infos: HashMap<PeerId, PeerInfo>,
    peer_addrs: HashMap<Multiaddr, PeerId>,
}

/// An in memory [`PeerBook`] implementation
#[derive(Default)]
pub struct MemoryPeerBook(Mutex<RawMemoryPeerBook>);

#[async_trait]
impl peerbook_syscall::DriverPeerBook for MemoryPeerBook {
    async fn insert(&self, mut info: PeerInfo) -> Result<Option<PeerInfo>> {
        log::trace!("MemoryPeerBook, put id={}", info.id);

        info.disappear = None;

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

    async fn appear(&self, peer_id: &PeerId, timestamp: SystemTime) -> Result<()> {
        if let Some(peer_info) = self.0.lock().await.peer_infos.get_mut(peer_id) {
            peer_info.appear = Some(timestamp);
        }

        Ok(())
    }

    async fn disappear(&self, peer_id: &PeerId, timestamp: SystemTime) -> Result<()> {
        if let Some(peer_info) = self.0.lock().await.peer_infos.get_mut(peer_id) {
            peer_info.disappear = Some(timestamp);
        }

        Ok(())
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

    async fn listen_on(&self, raddr: &Multiaddr) -> Result<Option<PeerId>> {
        Ok(self
            .0
            .lock()
            .await
            .peer_addrs
            .get(raddr)
            .map(|id| id.clone()))
    }
}
