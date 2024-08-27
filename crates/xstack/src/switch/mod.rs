use std::{
    fmt::Debug,
    ops::Deref,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use futures::{lock::Mutex, AsyncReadExt, AsyncWriteExt, TryStreamExt};
use futures_map::KeyWaitMap;
use identity::{PeerId, PublicKey};
use immutable::{ImmutableSwitch, SwitchBuilder};
use listener::ProtocolListener;
use multiaddr::Multiaddr;
use multistream_select::{dialer_select_proto, listener_select_proto, Version};
use mutable::MutableSwitch;
use protobuf::Message;
use rand::{seq::IteratorRandom, thread_rng};
use rasi::{task::spawn_ok, timer::TimeoutExt};

use crate::{
    book::{ConnectionType, PeerInfo},
    keystore::KeyStore,
    proto::identity::Identity,
    transport::{Listener, ProtocolStream, TransportConnection},
    Error, Result,
};

mod immutable;
mod listener;
mod mutable;
mod pool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ListenerId(usize);

impl ListenerId {
    fn next() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        ListenerId(NEXT.fetch_add(1, Ordering::SeqCst))
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
enum SwitchEvent {
    Accept(ListenerId),
}

/// protocol name of libp2p identity
pub const PROTOCOL_IPFS_ID: &str = "/ipfs/id/1.0.0";

/// protocol name of libp2p identity push
pub const PROTOCOL_IPFS_PUSH_ID: &str = "/ipfs/id/push/1.0.0";

/// protocol name of libp2p ping
pub const PROTOCOL_IPFS_PING: &str = "/ipfs/ping/1.0.0";

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

#[doc(hidden)]
pub struct InnerSwitch {
    public_key: PublicKey,
    local_peer_id: PeerId,
    immutable: ImmutableSwitch,
    mutable: Mutex<MutableSwitch>,
    event_map: KeyWaitMap<SwitchEvent, ()>,
}

impl Drop for InnerSwitch {
    fn drop(&mut self) {
        log::trace!("Switch dropping.");
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
    inner: Arc<InnerSwitch>,
}

impl Deref for Switch {
    type Target = InnerSwitch;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Switch {
    async fn handle_incoming(&self, listener: Listener) -> Result<()> {
        let mut incoming = listener.into_incoming();

        while let Some(mut conn) = incoming.try_next().await? {
            log::trace!(target:"switch","accept a new incoming connection, peer={}, local={}", conn.peer_addr(),conn.local_addr());

            let this = self.clone();

            spawn_ok(async move {
                if let Err(err) = this.setup_conn(&mut conn).await {
                    log::error!(target:"switch","setup connection, peer={}, local={}, err={}", conn.peer_addr(),conn.local_addr(),err);
                    _ = conn.close(&this).await;
                } else {
                    log::trace!(target:"switch","setup connection, peer={}, local={}", conn.peer_addr(),conn.local_addr());

                    this.mutable.lock().await.conn_pool.put(conn);
                }
            })
        }

        todo!()
    }

    async fn setup_conn(&self, conn: &mut TransportConnection) -> Result<()> {
        let this = self.clone();

        let mut this_conn = conn.clone();

        let peer_id = conn.public_key().to_peer_id();

        spawn_ok(async move {
            if let Err(err) = this.incoming_stream_loop(&mut this_conn).await {
                log::error!(target:"switch","incoming stream loop stopped, peer={}, local={}, error={}",this_conn.peer_addr(),this_conn.local_addr(),err);
                _ = this_conn.close(&this).await;
            } else {
                log::info!(target:"switch","incoming stream loop stopped, peer={}, local={}",this_conn.peer_addr(),this_conn.local_addr());
            }

            _ = this
                .update_conn_type(&peer_id, ConnectionType::CanConnect)
                .await;
        });

        // start "/ipfs/id/1.0.0" handshake.
        self.identity_request(conn)
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
                log::error!(target:"switch","dispatch stream, id={}, err={}", id, err);
            }
        }
    }

    async fn handle_incoming_stream(&self, mut stream: ProtocolStream) -> Result<()> {
        log::info!(target:"switch","accept new stream, peer={}, local={}, id={}",stream.peer_addr(),stream.local_addr(),stream.id());

        let protos = self.mutable.lock().await.protos();

        let (protoco_id, _) = listener_select_proto(&mut stream, &protos)
            .timeout(self.immutable.timeout)
            .await
            .ok_or(Error::Timeout)??;

        log::info!(target:"switch","protocol handshake, id={}, protocol={}, peer_id={}",stream.id(),protoco_id, stream.public_key().to_peer_id());

        let this = self.clone();
        let protoco_id = protoco_id.clone();

        spawn_ok(async move {
            let peer_addr = stream.peer_addr().clone();
            let local_addr = stream.local_addr().clone();
            let id = stream.id().to_owned();

            if let Err(err) = this.dispatch_stream(protoco_id, stream).await {
                log::error!(target:"switch","dispatch stream, id={}, peer={}, local={}, err={}",id, peer_addr,local_addr,err);
            } else {
                log::trace!(target:"switch","dispatch stream ok, id={}, peer={}, local={}",id, peer_addr, local_addr);
            }
        });

        Ok(())
    }

    async fn dispatch_stream(&self, protoco_id: String, stream: ProtocolStream) -> Result<()> {
        let conn_peer_id = stream.public_key().to_peer_id();

        match protoco_id.as_str() {
            PROTOCOL_IPFS_ID => self.identity_response(stream).await?,
            PROTOCOL_IPFS_PUSH_ID => self.identity_push(&conn_peer_id, stream).await?,
            PROTOCOL_IPFS_PING => self.ping_echo(stream).await?,
            _ => {
                let mut mutable = self.mutable.lock().await;

                if let Some(id) = mutable.insert_inbound_stream(stream, protoco_id) {
                    self.event_map.insert(SwitchEvent::Accept(id), ());
                }
            }
        }

        Ok(())
    }

    /// Handle `/ipfs/ping/1.0.0` request.
    async fn ping_echo(&self, mut stream: ProtocolStream) -> Result<()> {
        loop {
            log::trace!("recv /ipfs/ping/1.0.0");

            let body_len = unsigned_varint::aio::read_usize(&mut stream).await?;

            log::trace!("recv /ipfs/ping/1.0.0 payload len {}", body_len);

            if body_len != 32 {
                return Err(Error::InvalidPingLength(body_len));
            }

            let mut buf = vec![0; 31];

            stream.read_exact(&mut buf).await?;

            let mut payload_len = unsigned_varint::encode::usize_buffer();

            stream
                .write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
                .await?;

            stream.write_all(&buf).await?;

            log::trace!("send /ipfs/ping/1.0.0 echo");
        }
    }

    async fn identity_push(&self, conn_peer_id: &PeerId, mut stream: ProtocolStream) -> Result<()> {
        let identity = {
            log::trace!("identity_request: read varint length");

            let body_len = unsigned_varint::aio::read_usize(&mut stream).await?;

            log::trace!("identity_request: read varint length");

            if self.immutable.max_identity_packet_size < body_len {
                return Err(Error::IdentityOverflow(
                    self.immutable.max_identity_packet_size,
                ));
            }

            log::trace!("identity_request recv body: {}", body_len);

            let mut buf = vec![0; body_len];

            stream.read_exact(&mut buf).await?;

            Identity::parse_from_bytes(&buf)?
        };

        let pubkey = PublicKey::try_decode_protobuf(identity.publicKey())?;

        let peer_id = pubkey.to_peer_id();

        if *conn_peer_id != peer_id {
            return Err(Error::IdentityCheckFailed(*conn_peer_id, peer_id));
        }

        let raddrs = identity
            .listenAddrs
            .into_iter()
            .map(|buf| Multiaddr::try_from(buf).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;

        let observed_addrs = identity
            .observedAddr
            .into_iter()
            .map(|buf| Multiaddr::try_from(buf).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;

        //TODO: add nat codes
        log::info!(target:"switch","{} observed addrs: {:?}", peer_id, observed_addrs);

        let peer_info = PeerInfo {
            id: peer_id,
            addrs: raddrs,
            conn_type: ConnectionType::Connected,
        };

        self.add_peer(peer_info).await?;

        Ok(())
    }

    /// Start a "/ipfs/id/1.0.0" handshake.
    async fn identity_request(&self, conn: &mut TransportConnection) -> Result<()> {
        let mut stream = conn.connect().await?;

        let conn_peer_id = conn.public_key().to_peer_id();

        dialer_select_proto(&mut stream, ["/ipfs/id/1.0.0"], Version::V1).await?;

        self.identity_push(&conn_peer_id, stream).await
    }

    async fn identity_response(&self, mut stream: ProtocolStream) -> Result<()> {
        log::trace!("handle identity request");

        let peer_addr = stream.peer_addr();

        let mut identity = Identity::new();

        identity.set_observedAddr(peer_addr.to_vec());

        identity.set_publicKey(self.local_public_key().encode_protobuf());

        identity.set_agentVersion(self.immutable.agent_version.to_owned());

        identity.listenAddrs = self
            .local_addrs()
            .await
            .iter()
            .map(|addr| addr.to_vec())
            .collect::<Vec<_>>();

        identity.protocols = self.mutable.lock().await.protos();

        let buf = identity.write_to_bytes()?;

        let mut payload_len = unsigned_varint::encode::usize_buffer();

        stream
            .write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
            .await?;

        stream.write_all(&buf).await?;

        Ok(())
    }

    async fn transport_connect_prv(&self, raddr: &Multiaddr) -> Result<TransportConnection> {
        let transport = self
            .immutable
            .get_transport_by_address(raddr)
            .ok_or(Error::UnspportMultiAddr(raddr.to_owned()))?;

        log::trace!("{}, call transport driver", raddr);

        let mut conn = transport.connect(raddr, self.clone()).await?;

        log::trace!("{}, transport connection established", raddr);

        if let Err(err) = self.setup_conn(&mut conn).await {
            log::error!("{}, setup error: {}", raddr, err);
        } else {
            log::trace!("{}, setup success", raddr);
            self.mutable.lock().await.conn_pool.put(conn.clone());
        }

        Ok(conn)
    }

    /// Create a new connection to peer by id.
    ///
    /// This function will first check for a local connection cache,
    /// and if there is one, it will directly return the cached connection
    async fn transport_connect_to(&self, id: &PeerId) -> Result<TransportConnection> {
        log::trace!("{}, connect", id);

        if let Some(conns) = self.mutable.lock().await.conn_pool.get(id) {
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
            log::trace!("{}, connect to {}", id, raddr);

            match self.transport_connect_prv(&raddr).await {
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

        self.update_conn_type(id, ConnectionType::CannotConnect)
            .await?;

        Err(last_error.unwrap_or(Error::ConnectPeer(id.to_owned())))
    }

    pub(crate) async fn remove_conn(&self, conn: &TransportConnection) {
        _ = self.mutable.lock().await.conn_pool.remove(conn);
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
    pub async fn transport_connect(&self, raddr: &Multiaddr) -> Result<TransportConnection> {
        log::trace!("{}, try establish transport connection", raddr);

        if let Some(peer_id) = self.peer_id_of(raddr).await? {
            log::trace!(
                "{}, found peer_id in local book, peer_id={}",
                raddr,
                peer_id
            );

            return self.transport_connect_to(&peer_id).await;
        }

        self.transport_connect_prv(raddr).await
    }

    /// Create a new transport layer socket that accepts peer's inbound connections.
    pub async fn transport_bind(&self, laddr: &Multiaddr) -> Result<()> {
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
            ConnectTo::PeerIdRef(peer_id) => self.transport_connect_to(peer_id).await?,
            ConnectTo::MultiaddrRef(raddr) => self.transport_connect(raddr).await?,
            ConnectTo::PeerId(peer_id) => self.transport_connect_to(&peer_id).await?,
            ConnectTo::Multiaddr(raddr) => self.transport_connect(&raddr).await?,
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
    /// Remove [`PeerInfo`] from the [`PeerBook`] of this switch.
    pub async fn remove_peer(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        Ok(self.immutable.peer_book.remove(peer_id).await?)
    }

    /// Update the [`PeerBook`] of this switch.
    pub async fn add_peer(&self, peer_info: PeerInfo) -> Result<Option<PeerInfo>> {
        Ok(self.immutable.peer_book.put(peer_info).await?)
    }

    /// Update the connection type of the [`peer_id`](PeerId) in the [`PeerBook`] of this switch.
    pub async fn update_conn_type(
        &self,
        peer_id: &PeerId,
        conn_type: ConnectionType,
    ) -> Result<Option<ConnectionType>> {
        Ok(self
            .immutable
            .peer_book
            .update_conn_type(peer_id, conn_type)
            .await?)
    }

    /// Returns the [`PeerInfo`] of the [`peer_id`](PeerId).
    pub async fn peer_info(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        Ok(self.immutable.peer_book.get(peer_id).await?)
    }

    /// Returns the [`PeerId`] of the [`raddr`](Multiaddr).
    pub async fn peer_id_of(&self, raddr: &Multiaddr) -> Result<Option<PeerId>> {
        Ok(self.immutable.peer_book.peer_id_of(raddr).await?)
    }

    /// Get associated keystore instance.
    pub fn keystore(&self) -> &KeyStore {
        &self.immutable.keystore
    }

    /// Get this switch's public key.
    pub fn local_public_key(&self) -> &PublicKey {
        &self.inner.public_key
    }

    /// Get this switch's node id.
    pub fn local_id(&self) -> &PeerId {
        &self.inner.local_peer_id
    }

    /// Returns the addresses list of this switch is bound to.
    pub async fn local_addrs(&self) -> Vec<Multiaddr> {
        self.mutable.lock().await.local_addrs()
    }

    /// Register self into global context.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub fn into_global(self) {
        use crate::register_switch;

        register_switch(self)
    }
}
