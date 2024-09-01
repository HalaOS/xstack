use std::{
    fmt::Debug,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use super::{immutable::ImmutableSwitch, AutoNAT, PROTOCOL_IPFS_ID, PROTOCOL_IPFS_PING};
use super::{mutable::MutableSwitch, PROTOCOL_IPFS_PUSH_ID};
use futures::{lock::Mutex, TryStreamExt};
use futures_map::KeyWaitMap;
use libp2p_identity::{PeerId, PublicKey};
use multiaddr::Multiaddr;
use multistream_select::{dialer_select_proto, listener_select_proto, Version};

use rand::{seq::IteratorRandom, thread_rng};
use rasi::{task::spawn_ok, timer::TimeoutExt};

use crate::{
    book::PeerInfo,
    event::{Event, EventSource},
    keystore::KeyStore,
    transport::{ProtocolStream, TransportConnection, TransportListener},
    Error, Result,
};

pub use super::immutable::SwitchBuilder;
pub use super::listener::ProtocolListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ListenerId(usize);

impl From<ListenerId> for usize {
    fn from(value: ListenerId) -> Self {
        value.0
    }
}

impl From<&ListenerId> for usize {
    fn from(value: &ListenerId) -> Self {
        value.0
    }
}

impl ListenerId {
    pub(super) fn next() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        ListenerId(NEXT.fetch_add(1, Ordering::SeqCst))
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub(super) enum SwitchInnerEvent {
    Accept(ListenerId),
}

/// Variant type used by [`connect`](Switch::connect) function.
pub enum ConnectTo<'a> {
    PeerIdRef(&'a PeerId),
    MultiaddrRef(&'a Multiaddr),
    PeerId(PeerId),
    Multiaddr(Multiaddr),
}

impl<'a> From<&'a PeerId> for ConnectTo<'a> {
    fn from(value: &'a PeerId) -> Self {
        Self::PeerIdRef(value)
    }
}

impl<'a> From<&'a Multiaddr> for ConnectTo<'a> {
    fn from(value: &'a Multiaddr) -> Self {
        Self::MultiaddrRef(value)
    }
}

impl From<PeerId> for ConnectTo<'static> {
    fn from(value: PeerId) -> Self {
        Self::PeerId(value)
    }
}

impl From<Multiaddr> for ConnectTo<'static> {
    fn from(value: Multiaddr) -> Self {
        Self::Multiaddr(value)
    }
}

impl TryFrom<&str> for ConnectTo<'static> {
    type Error = Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        if let Ok(peer_id) = value.parse::<PeerId>() {
            return Ok(Self::PeerId(peer_id));
        }

        return Ok(Self::Multiaddr(value.parse::<Multiaddr>()?));
    }
}

/// `Switch` is the entry point of the libp2p network.
///
/// via `Switch` instance, you can:
/// - create a outbound stream to peer.
/// - accept a inbound stream from peer.
///
/// # Multiaddr hit
#[derive(Clone)]
pub struct Switch {
    // pub(super) inner: Arc<InnerSwitch>,
    pub(super) public_key: Arc<PublicKey>,
    pub(super) local_peer_id: Arc<PeerId>,
    pub(super) immutable: Arc<ImmutableSwitch>,
    pub(super) mutable: Arc<Mutex<MutableSwitch>>,
    pub(super) event_map: Arc<KeyWaitMap<SwitchInnerEvent, ()>>,
}

// impl Deref for Switch {
//     type Target = InnerSwitch;

//     fn deref(&self) -> &Self::Target {
//         &self.inner
//     }
// }

impl Switch {
    async fn handle_incoming(&self, listener: TransportListener) -> Result<()> {
        let mut incoming = listener.into_incoming();

        while let Some(mut conn) = incoming.try_next().await? {
            log::trace!(
                "accept a new incoming connection, peer={}, local={}",
                conn.peer_addr(),
                conn.local_addr()
            );

            let this = self.clone();

            spawn_ok(async move {
                if let Err(err) = this.handshake(&mut conn, false).await {
                    log::error!(
                        "setup connection, peer={}, local={}, err={}",
                        conn.peer_addr(),
                        conn.local_addr(),
                        err
                    );
                    _ = conn.close(&this).await;
                } else {
                    log::trace!(
                        "setup connection, peer={}, local={}",
                        conn.peer_addr(),
                        conn.local_addr()
                    );
                }
            })
        }

        Ok(())
    }

    /// Start a background task to accept inbound stream, and make a identity request to authenticate peer.
    async fn handshake(&self, conn: &mut TransportConnection, pin: bool) -> Result<()> {
        self.mutable.lock().await.start_conn_handshake(conn);

        let this = self.clone();

        let mut this_conn = conn.clone();

        spawn_ok(async move {
            if let Err(err) = this.incoming_stream_loop(&mut this_conn).await {
                log::error!(
                    "incoming stream loop stopped, peer={}, local={}, error={}",
                    this_conn.peer_addr(),
                    this_conn.local_addr(),
                    err
                );
                _ = this_conn.close(&this).await;
            } else {
                log::info!(
                    "incoming stream loop stopped, peer={}, local={}",
                    this_conn.peer_addr(),
                    this_conn.local_addr()
                );
            }
        });

        // start "/ipfs/id/1.0.0" handshake.
        self.identity_request(conn, pin)
            .timeout(self.immutable.timeout)
            .await
            .ok_or(Error::Timeout)??;

        Ok(())
    }

    async fn incoming_stream_loop(&self, conn: &mut TransportConnection) -> Result<()> {
        loop {
            let stream = conn.accept().await?;

            let id = stream.id().to_owned();

            if let Err(err) = self.handle_incoming_stream(stream).await {
                log::trace!("dispatch stream, id={}, err={}", id, err);
            }
        }
    }

    async fn handle_incoming_stream(&self, mut stream: ProtocolStream) -> Result<()> {
        log::info!(
            "accept new stream, peer={}, local={}, id={}",
            stream.peer_addr(),
            stream.local_addr(),
            stream.id()
        );

        let protos = self.mutable.lock().await.protos();

        let (protoco_id, _) = listener_select_proto(&mut stream, &protos)
            .timeout(self.immutable.timeout)
            .await
            .ok_or(Error::Timeout)??;

        log::trace!(
            "protocol handshake, id={}, protocol={}, peer_id={}",
            stream.id(),
            protoco_id,
            stream.public_key().to_peer_id()
        );

        let this = self.clone();
        let protoco_id = protoco_id.clone();

        spawn_ok(async move {
            let peer_addr = stream.peer_addr().clone();
            let local_addr = stream.local_addr().clone();
            let id = stream.id().to_owned();

            if let Err(err) = this.dispatch_stream(protoco_id, stream).await {
                log::error!(
                    "dispatch stream, id={}, peer={}, local={}, err={}",
                    id,
                    peer_addr,
                    local_addr,
                    err
                );
            } else {
                log::trace!(
                    "dispatch stream ok, id={}, peer={}, local={}",
                    id,
                    peer_addr,
                    local_addr
                );
            }
        });

        Ok(())
    }

    async fn dispatch_stream(&self, protoco_id: String, stream: ProtocolStream) -> Result<()> {
        let conn_peer_id = stream.public_key().to_peer_id();

        match protoco_id.as_str() {
            PROTOCOL_IPFS_ID => self.identity_response(stream).await?,
            PROTOCOL_IPFS_PUSH_ID => {
                self.identity_push(&conn_peer_id, stream).await?;
            }
            PROTOCOL_IPFS_PING => self.ping_echo(stream).await?,
            _ => {
                let mut mutable = self.mutable.lock().await;

                if let Some(id) = mutable.insert_inbound_stream(stream, protoco_id) {
                    self.event_map.insert(SwitchInnerEvent::Accept(id), ());
                }
            }
        }

        Ok(())
    }

    async fn transport_connect_prv(
        &self,
        raddr: &Multiaddr,
        pin: bool,
    ) -> Result<TransportConnection> {
        let transport = self
            .immutable
            .get_transport_by_address(raddr)
            .ok_or(Error::UnspportMultiAddr(raddr.to_owned()))?;

        log::trace!("{}, call transport driver", raddr);

        let mut conn = transport.connect(raddr, self.clone()).await?;

        log::trace!("{}, transport connection established", raddr);

        if let Err(err) = self.handshake(&mut conn, pin).await {
            log::error!("{}, setup error: {}", raddr, err);
            _ = conn.close(self).await;
        } else {
            log::trace!("{}, setup success", raddr);
        }

        Ok(conn)
    }

    /// Create a new connection to peer by id.
    ///
    /// This function will first check for a local connection cache,
    /// and if there is one, it will directly return the cached connection
    async fn transport_connect_to(&self, id: &PeerId, pin: bool) -> Result<TransportConnection> {
        log::trace!("{}, connect", id);

        if let Some(conns) = self.mutable.lock().await.get_conn(id) {
            if !conns.is_empty() {
                log::trace!("{}, reused connection in local pool.", id);
                let conn = conns.into_iter().choose(&mut thread_rng()).unwrap();
                return Ok(conn);
            }
        }

        let peer_info = self
            .immutable
            .peer_book
            .get(id)
            .await?
            .ok_or(Error::ConnectPeer(id.clone()))?;

        let mut last_error = None;

        for raddr in peer_info.addrs {
            let raddr = match raddr.with_p2p(id.clone()) {
                Ok(raddr) => raddr,
                Err(raddr) => raddr,
            };

            log::trace!("connect to {}", raddr);

            match self.transport_connect_prv(&raddr, pin).await {
                Ok(conn) => {
                    log::trace!("{}, connect to {}, established", id, raddr);
                    return Ok(conn);
                }
                Err(err) => {
                    last_error = {
                        log::trace!("{}, connect to {}, error: {}", id, raddr, err);
                        Some(err)
                    }
                }
            }
        }

        Err(last_error.unwrap_or(Error::ConnectPeer(id.to_owned())))
    }

    pub(crate) async fn remove_conn(&self, conn: &TransportConnection) {
        self.mutable.lock().await.remove_conn(conn);

        let peer_id = conn.public_key().to_peer_id();

        if let Err(err) = self
            .immutable
            .peer_book
            .disappear(&peer_id, SystemTime::now())
            .await
        {
            log::error!(
                "Failed to update peer's disappearance timestamp, id={}, err={}",
                peer_id,
                err
            );
        }
    }

    /// Create a new transport layer socket that accepts peer's inbound connections.
    ///
    pub(crate) async fn transport_bind(&self, laddr: &Multiaddr) -> Result<()> {
        let transport = self
            .immutable
            .get_transport_by_address(laddr)
            .ok_or(Error::UnspportMultiAddr(laddr.to_owned()))?;

        let listener = transport.bind(laddr, self.clone()).await?;

        let laddr = listener.local_addr()?;

        self.mutable.lock().await.transport_bind_to(laddr.clone());

        let this = self.clone();

        spawn_ok(async move {
            if let Err(err) = this.handle_incoming(listener).await {
                log::error!(target:"switch" ,"listener({}) stop, err={}",laddr, err);
            } else {
                log::info!(target:"switch" ,"listener({}) stop",laddr);
            }
        });

        Ok(())
    }
}

impl Switch {
    /// Uses `agent_version` string to create a switch [`builder`](SwitchBuilder).
    pub fn new<A>(agent_version: A) -> SwitchBuilder
    where
        A: AsRef<str>,
    {
        SwitchBuilder::new(agent_version.as_ref().to_owned())
    }

    /// Connect to peer with provided [`raddr`](Multiaddr).
    ///
    /// This function first query the route table to get the peer id,
    /// if exists then check for a local connection cache.
    ///
    /// if the parameter pin is true, the `Switch` will not drop the created connection when the connection pool is doing garbage collect
    pub async fn transport_connect(
        &self,
        raddr: &Multiaddr,
        pin: bool,
    ) -> Result<TransportConnection> {
        log::trace!("{}, try establish transport connection", raddr);

        if let Some(peer_id) = self.lookup_peer_id(raddr).await? {
            log::trace!(
                "{}, found peer_id in local book, peer_id={}",
                raddr,
                peer_id
            );

            return self.transport_connect_to(&peer_id, pin).await;
        }

        self.transport_connect_prv(raddr, pin).await
    }

    /// Create a protocol layer server-side socket, that accept inbound [`ProtocolStream`].
    pub async fn bind<I>(&self, protos: I) -> Result<ProtocolListener>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let id = self.mutable.lock().await.new_protocol_listener(protos)?;

        Ok(ProtocolListener::new(id, self.clone()))
    }

    /// Connect to peer and negotiate a protocol. the `protos` is the list of candidate protocols.
    pub async fn connect<'a, C, E, I>(
        &self,
        target: C,
        protos: I,
    ) -> Result<(ProtocolStream, String)>
    where
        C: TryInto<ConnectTo<'a>, Error = E>,
        I: IntoIterator,
        I::Item: AsRef<str>,
        E: Debug,
    {
        let mut conn = match target
            .try_into()
            .map_err(|err| Error::Other(format!("{:?}", err)))?
        {
            ConnectTo::PeerIdRef(peer_id) => self.transport_connect_to(peer_id, false).await?,
            ConnectTo::MultiaddrRef(raddr) => self.transport_connect(raddr, false).await?,
            ConnectTo::PeerId(peer_id) => self.transport_connect_to(&peer_id, false).await?,
            ConnectTo::Multiaddr(raddr) => self.transport_connect(&raddr, false).await?,
        };

        log::trace!("open stream, conn_id={}", conn.id());
        let mut stream = conn.connect().await?;

        log::trace!("dial select proto, conn_id={}", conn.id());

        let (protocol_id, _) = dialer_select_proto(&mut stream, protos, Version::V1)
            .timeout(self.immutable.timeout)
            .await
            .ok_or(Error::Timeout)??;

        Ok((stream, protocol_id.as_ref().to_owned()))
    }
}

impl Switch {
    /// Create a new [`Event`] listener.
    pub async fn on<E>(&self, buffer: usize) -> EventSource<E>
    where
        E: Event,
    {
        self.mutable.lock().await.new_listener(buffer)
    }

    /// Remove [`PeerInfo`] from the [`PeerBook`](crate::book::PeerBook) of this switch.
    pub async fn remove_peer_info(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        Ok(self.immutable.peer_book.remove(peer_id).await?)
    }

    /// insert new [`PeerInfo`] into the [`PeerBook`](crate::PeerBook) of this `Switch`
    pub async fn insert_peer_info(&self, peer_info: PeerInfo) -> Result<Option<PeerInfo>> {
        Ok(self.immutable.peer_book.insert(peer_info).await?)
    }

    /// Returns the [`PeerInfo`] of the [`peer_id`](PeerId).
    pub async fn lookup_peer_info(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        Ok(self.immutable.peer_book.get(peer_id).await?)
    }

    /// Reverse lookup [`PeerId`] for the peer indicated by the listening address.
    pub async fn lookup_peer_id(&self, raddr: &Multiaddr) -> Result<Option<PeerId>> {
        Ok(self.immutable.peer_book.listen_on(raddr).await?)
    }

    /// Get associated keystore instance.
    pub fn keystore(&self) -> &KeyStore {
        &self.immutable.keystore
    }

    /// Get this switch's public key.
    pub fn local_public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// Get this switch's node id.
    pub fn local_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// Returns the addresses list of this switch is bound to.
    pub async fn local_addrs(&self) -> Vec<Multiaddr> {
        self.mutable.lock().await.local_addrs()
    }

    /// Returns the addresses list of this switch is listen to.
    ///
    /// Unlike the [`local_addrs`](Self::local_addrs) function, this function may returns circuit-v2 addresses.
    pub async fn listen_addrs(&self) -> Vec<Multiaddr> {
        self.mutable.lock().await.listen_addrs()
    }

    /// Sets the list of listening addresses for the [`circuit-v2/stop`] protocol.
    /// to change listening addresses to circuit protocol addresses.
    ///
    /// [`circuit-v2/stop`]: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md#stop-protocol
    pub async fn set_nat_addrs(&self, addrs: Vec<Multiaddr>) {
        self.mutable.lock().await.set_net_addrs(addrs);
    }

    /// Returns the [*autonat protocol*](https://github.com/libp2p/specs/tree/master/autonat) [`state`](AutoNAT).
    pub async fn nat(&self) -> AutoNAT {
        self.mutable.lock().await.auto_nat()
    }

    /// Set the the [*autonat protocol*](https://github.com/libp2p/specs/tree/master/autonat) [`state`](AutoNAT).
    pub async fn set_nat(&self, state: AutoNAT) {
        self.mutable.lock().await.set_nat(state)
    }

    /// Returns the `max_packet_size` configuration value.
    pub fn max_packet_size(&self) -> usize {
        self.immutable.max_packet_size
    }

    /// Returns the protocol `timeout` configuration value.
    pub fn timeout(&self) -> Duration {
        self.immutable.timeout
    }

    /// Register self into global context.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub fn into_global(self) {
        use crate::register_switch;

        register_switch(self)
    }
}
