use std::ops::Deref;
use std::{fmt::Debug, sync::Arc};

use super::SwitchBuilder;
use super::{builder::SwitchOptions, PROTOCOL_IPFS_ID, PROTOCOL_IPFS_PING};
use super::{mutable::MutableSwitch, PROTOCOL_IPFS_PUSH_ID};

use futures::{lock::Mutex, TryStreamExt};
use libp2p_identity::{PeerId, PublicKey};
use multiaddr::Multiaddr;
use multistream_select::listener_select_proto;

use rasi::{task::spawn_ok, timer::TimeoutExt};

use crate::{
    book::PeerInfo,
    transport::{P2pConn, ProtocolStream, TransportListener},
    Error, ProtocolListener, Result, XStackId,
};
use crate::{AutoNAT, ConnectTo};

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
    pub(super) ops: Arc<SwitchOptions>,
    pub(super) mutable: Arc<Mutex<MutableSwitch>>,
}

impl Deref for Switch {
    type Target = SwitchOptions;

    fn deref(&self) -> &Self::Target {
        &self.ops
    }
}

impl Switch {
    /// Start a background task to accept inbound stream, and make a identity request to authenticate peer.
    async fn handshake(&self, conn: &mut P2pConn) -> Result<()> {
        self.ops.stream_dispatcher.handshake(conn.id()).await;

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
                _ = this_conn.close();
            } else {
                log::info!(
                    "incoming stream loop stopped, peer={}, local={}",
                    this_conn.peer_addr(),
                    this_conn.local_addr()
                );
            }
        });

        // start "/ipfs/id/1.0.0" handshake.
        self.identity_request(conn)
            .timeout(self.ops.timeout)
            .await
            .ok_or(Error::Timeout)??;

        Ok(())
    }

    async fn incoming_stream_loop(&self, conn: &mut P2pConn) -> Result<()> {
        loop {
            let stream = conn.accept().await?;

            let this = self.clone();

            spawn_ok(async move {
                let id = stream.id().to_owned();

                if let Err(err) = this.handle_incoming_stream(stream).await {
                    log::trace!("dispatch stream, id={}, err={}", id, err);
                }
            })
        }
    }

    async fn handle_incoming_stream(&self, mut stream: ProtocolStream) -> Result<()> {
        log::info!(
            "accept new stream, peer={}, local={}, id={}",
            stream.peer_addr(),
            stream.local_addr(),
            stream.id()
        );

        let protos = Self::merge_protos(self.ops.stream_dispatcher.protos().await);

        let (protoco_id, _) = listener_select_proto(&mut stream, &protos)
            .timeout(self.ops.timeout)
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

        Ok(())
    }

    async fn dispatch_stream(&self, protocol_id: String, stream: ProtocolStream) -> Result<()> {
        let conn_peer_id = stream.public_key().to_peer_id();

        match protocol_id.as_str() {
            PROTOCOL_IPFS_ID => self.identity_response(stream).await?,
            PROTOCOL_IPFS_PUSH_ID => {
                self.identity_push(&conn_peer_id, stream).await?;
            }
            PROTOCOL_IPFS_PING => self.ping_echo(stream).await?,
            _ => {
                self.ops
                    .stream_dispatcher
                    .dispatch(stream, protocol_id)
                    .await;
            }
        }

        Ok(())
    }

    /// Create a new transport layer socket that accepts peer's inbound connections.
    ///
    pub(crate) async fn transport_bind(&self, laddr: &Multiaddr) -> Result<()> {
        let transport = self
            .ops
            .get_transport_by_address(laddr)
            .ok_or(Error::UnspportMultiAddr(laddr.to_owned()))?;

        let listener = transport.bind(self, laddr).await?;

        let laddr = listener.local_addr()?;

        self.mutable.lock().await.transport_bind_to(laddr.clone());

        self.transport_bind_with(listener).await
    }

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
                if let Err(err) = this.handshake(&mut conn).await {
                    log::error!(
                        "setup connection, peer={}, local={}, err={}",
                        conn.peer_addr(),
                        conn.local_addr(),
                        err
                    );
                    _ = conn.close();
                } else {
                    log::trace!(
                        "setup connection, peer={}, local={}",
                        conn.peer_addr(),
                        conn.local_addr()
                    );

                    this.connector.incoming(conn).await;
                }
            })
        }

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

    /// Open a new transport connection directly to peer via `raddr`.
    ///
    /// **Note!!, that this method does not involve connection reuse,
    /// so it is safe to call it in implementations such as [`Connector`]**
    ///
    /// [`Connector`]: crate::connector_syscall::DriverConnector
    pub async fn transport_connect(&self, raddr: &Multiaddr) -> Result<P2pConn> {
        let transport = self
            .ops
            .get_transport_by_address(raddr)
            .ok_or(Error::UnspportMultiAddr(raddr.to_owned()))?;

        let mut conn = transport
            .connect(self, raddr)
            .timeout(self.timeout)
            .await
            .ok_or(Error::Timeout)??;

        if let Err(err) = self.handshake(&mut conn).await {
            log::error!("{}, setup error: {}", raddr, err);
            _ = conn.close();
            Err(err)
        } else {
            log::trace!("{}, setup success", raddr);
            Ok(conn)
        }
    }

    /// Dynamically bind a transport listener to this switch.
    pub async fn transport_bind_with(&self, listener: TransportListener) -> Result<()> {
        let this = self.clone();

        let laddr = listener.local_addr()?;

        spawn_ok(async move {
            if let Err(err) = this.handle_incoming(listener).await {
                log::error!("listener({}) stop, err={}", laddr, err);
            } else {
                log::info!("listener({}) stop", laddr);
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
        let id = XStackId::default();

        let protos = protos
            .into_iter()
            .map(|item| item.as_ref().to_owned())
            .collect::<Vec<_>>();

        self.ops.stream_dispatcher.bind(id, &protos).await?;

        Ok(ProtocolListener::new(self.clone(), id))
    }

    /// Connect to peer and negotiate a protocol. the `protos` is the list of candidate protocols.
    pub async fn connect<'a, C, E, I>(
        &self,
        target: C,
        protos: I,
    ) -> Result<(ProtocolStream, I::Item)>
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
            ConnectTo::PeerIdRef(peer_id) => self.ops.connector.connect_to(self, peer_id).await?,
            ConnectTo::MultiaddrRef(raddr) => {
                self.ops.connector.connect(self, &[raddr.clone()]).await?
            }
            ConnectTo::PeerId(peer_id) => self.ops.connector.connect_to(self, &peer_id).await?,
            ConnectTo::Multiaddr(raddr) => {
                self.ops.connector.connect(self, &[raddr.clone()]).await?
            }
            ConnectTo::MultiaddrsRef(raddrs) => self.ops.connector.connect(self, &raddrs).await?,
            ConnectTo::Multiaddrs(raddrs) => self.ops.connector.connect(self, &raddrs).await?,
        };

        log::trace!("open stream, conn_id={}", conn.id());

        Ok(conn.connect(protos).await?)
    }
}

impl Switch {
    /// Remove [`PeerInfo`] from the [`PeerBook`](crate::book::PeerBook) of this switch.
    pub async fn remove_peer_info(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        Ok(self.ops.peer_book.remove(peer_id).await?)
    }

    /// insert new [`PeerInfo`] into the [`PeerBook`](crate::PeerBook) of this `Switch`
    pub async fn insert_peer_info(&self, peer_info: PeerInfo) -> Result<Option<PeerInfo>> {
        Ok(self.ops.peer_book.insert(peer_info).await?)
    }

    /// Returns the [`PeerInfo`] of the [`peer_id`](PeerId).
    pub async fn lookup_peer_info(&self, peer_id: &PeerId) -> Result<Option<PeerInfo>> {
        Ok(self.ops.peer_book.get(peer_id).await?)
    }

    /// Random select peers by protoco_id.
    pub async fn choose_peers<P: AsRef<str>>(
        &self,
        protocol_id: P,
        limits: usize,
    ) -> Result<Vec<PeerId>> {
        Ok(self
            .ops
            .peer_book
            .choose_peers(protocol_id.as_ref(), limits)
            .await?)
    }

    /// Reverse lookup [`PeerId`] for the peer indicated by the listening address.
    pub async fn lookup_peer_id(&self, raddr: &Multiaddr) -> Result<Option<PeerId>> {
        Ok(self.ops.peer_book.listen_on(raddr).await?)
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

    /// Returns the addresses list of this switch observed by peers.
    pub async fn observed_addrs(&self) -> Vec<Multiaddr> {
        self.mutable.lock().await.observed_addrs()
    }

    ///  Insert observed addresses list.
    pub async fn set_observed_addrs(&self, addrs: Vec<Multiaddr>) {
        self.mutable.lock().await.set_observed_addrs(addrs)
    }

    /// Returns the addresses list of this switch is listen to.
    ///
    /// Unlike the [`local_addrs`](Self::local_addrs) function, this function may returns circuit-v2 addresses.
    pub async fn listen_addrs(&self) -> Vec<Multiaddr> {
        self.mutable.lock().await.listen_addrs()
    }

    /// Sets the [`circuit-v2`] listening addresses.
    ///
    /// [`circuit-v2/stop`]: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md#stop-protocol
    pub async fn set_nat_addrs(&self, addrs: Vec<Multiaddr>) {
        self.mutable.lock().await.set_net_addrs(addrs);
    }

    /// remove [`circuit-v2`] listening addresses from this `Switch`.
    pub async fn remove_nat_addrs(&self, addrs: &[Multiaddr]) {
        self.mutable.lock().await.remove_net_addrs(addrs);
    }

    /// Returns the [*autonat protocol*](https://github.com/libp2p/specs/tree/master/autonat) [`state`](AutoNAT).
    pub async fn nat(&self) -> AutoNAT {
        self.mutable.lock().await.auto_nat()
    }

    /// Set the the [*autonat protocol*](https://github.com/libp2p/specs/tree/master/autonat) [`state`](AutoNAT).
    pub async fn set_nat(&self, state: AutoNAT) {
        self.mutable.lock().await.set_nat(state);
        self.event_mediator
            .raise(crate::Event::Network(state))
            .await;
    }

    /// Register self into global context.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub fn into_global(self) {
        use crate::register_switch;

        register_switch(self)
    }
}
