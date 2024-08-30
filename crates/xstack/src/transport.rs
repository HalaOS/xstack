//! A plugin system for libp2p's transport layer protocol.

use std::{io::Result, pin::Pin};

use futures::{stream::unfold, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use rand::{thread_rng, RngCore};

use crate::{driver_wrapper, switch::Switch, Error, PROTOCOL_IPFS_PING};

/// A libp2p transport driver must implement the `Driver-*` traits in this module.
///
///
/// This mod is the core of the **XSTACK** modularity,
/// all of the [*transport protocols*](https://docs.libp2p.io/concepts/transports/overview/)
/// that defined by libp2p are developed through this mod.
/// unlike [`rust-libp2p`](https://docs.rs/libp2p/latest/libp2p/trait.Transport.html),
/// we use the [**async_trait**](https://docs.rs/async-trait/) crate to define the
/// [`transport`](transport_syscall::DriverTransport) trait as a way to make it less difficult to understand:
///
/// ## Register
///
/// Developers can register a customise `Transport` by:
/// ```no_run
/// use xstack::Switch;
///
/// # async fn boostrap() {
/// Switch::new("test")
///       // .transport(NoopTransport)
///       .create()
///       .await
///       .unwrap()
///       // register to global context.
///       .into_global();
/// # }
///```
///
/// The `NoopTransport` is a structure that implement [`DriverTransport`](transport_syscall::DriverTransport) trait.
///
/// ## Lifecycle
///
/// After register, the `Switch` take over the lifecycle of the `Transport`:
///
/// ###  When a *connect* request is arrived
///
/// - The `Switch` loop the whole registered *Transports* list
/// and call their [`multiaddr_hit`](transport_syscall::DriverTransport::multiaddr_hit) function;
/// when a *Transport* returns true,  stops the loop process and immediately calls its
/// [`connect`](transport_syscall::DriverTransport::connect) function to create a new
/// [*transport connection*](transport_syscall::DriverConnection);
///
/// - The created [**Connection**](transport_syscall::DriverConnection)
/// is hosted by an internal connection pool of the `Switch`. and all responses for these requests
/// from the peer (e.g., [ping](https://github.com/libp2p/specs/blob/master/ping/ping.md),
/// [identity](https://github.com/libp2p/specs/tree/master/identify),
/// [identity/push](https://github.com/libp2p/specs/tree/master/identify#identifypush),
/// etc.) are taken over by it.
///
/// - And then, the `Switch` send an *identity* request to the peer via a stream opened on the *created connection*.
/// after the identity of the peer is confirmed, the `Switch`
/// [**negotiates**](https://github.com/libp2p/specs/blob/master/connections/README.md#multistream-select)
/// a requested [`ProtocolStream`](crate::ProtocolStream) with the peer and returns it to the caller.
///
///
/// ### When a *bind* request is arrived
///
/// - The `Switch` loop the whole registered *Transports* list
/// and call their [`multiaddr_hit`](transport_syscall::DriverTransport::multiaddr_hit) function;
/// when a *Transport* returns true,  stops the loop process and immediately calls its
/// [`bind`](transport_syscall::DriverTransport::bind) function to create a new
/// [*transport listener*](transport_syscall::DriverListener);
///
/// - The created [**Listener**](transport_syscall::DriverListener) is handled by an internal loop task,
/// and the inbound [**Connection**](transport_syscall::DriverConnection) accepted by it
/// is hosted by an internal connection pool of the `Switch`. and all responses for these requests
/// from the peer (e.g., [ping](https://github.com/libp2p/specs/blob/master/ping/ping.md),
/// [identity](https://github.com/libp2p/specs/tree/master/identify),
/// [identity/push](https://github.com/libp2p/specs/tree/master/identify#identifypush),
/// etc.) are taken over by the `Switch`.
///
/// - The other requests are dispatch to [`ProtocolListener`](crate::ProtocolListener) by protocol types,
/// you can create a *protocol listener* like this:
///
/// ```no_run
/// use xstack::ProtocolListener;
/// use futures::TryStreamExt;
///
/// # async fn boostrap() {
/// let mut incoming = ProtocolListener::bind(["/ipfs/kad/1.0.0"]).await.unwrap().into_incoming();
///
/// while let Some((stream,_)) = incoming.try_next().await.unwrap() {
///     // handle rpc request.
/// }
/// # }
/// ```
///
/// ***the parameter passed to the [`bind`](crate::ProtocolListener::bind) function is a protocol list that the listener can accept***
pub mod transport_syscall {
    use std::{
        io::Result,
        task::{Context, Poll},
    };

    use async_trait::async_trait;
    use libp2p_identity::PublicKey;
    use multiaddr::Multiaddr;

    use crate::switch::Switch;

    use super::*;

    /// The core trait of the [`Transports`](https://docs.libp2p.io/concepts/transports/overview/).
    #[async_trait]
    pub trait DriverTransport: Send + Sync {
        /// Create a server-side socket with provided [`laddr`](Multiaddr).
        async fn bind(&self, laddr: &Multiaddr, switch: Switch) -> Result<TransportListener>;

        /// Connect to peer with remote peer [`raddr`](Multiaddr).
        async fn connect(&self, raddr: &Multiaddr, switch: Switch) -> Result<TransportConnection>;

        /// Check if this transport support the protocol stack represented by the `addr`.
        fn multiaddr_hit(&self, addr: &Multiaddr) -> bool;
    }

    /// A server-side socket that accept new incoming stream.
    #[async_trait]
    pub trait DriverListener: Sync + Sync {
        /// Accept next incoming connection between local and peer.
        async fn accept(&mut self) -> Result<TransportConnection>;

        /// Returns the local address that this listener is bound to.
        fn local_addr(&self) -> Result<Multiaddr>;
    }

    /// A secure communication channel that support [*stream muliplexing*](https://docs.libp2p.io/concepts/multiplex/overview/)
    #[async_trait]
    pub trait DriverConnection: Send + Sync + Unpin {
        /// The app scope unique id for this driver connection .
        fn id(&self) -> &str;

        /// Return the remote peer's public key.
        fn public_key(&self) -> &PublicKey;

        /// Returns the local address that this stream is bound to.
        fn local_addr(&self) -> &Multiaddr;

        /// Returns the remote address that this stream is connected to.
        fn peer_addr(&self) -> &Multiaddr;

        /// Accept a new incoming stream with protocol selection.
        async fn accept(&mut self) -> Result<super::ProtocolStream>;

        /// Create a new outbound stream with protocol selection
        async fn connect(&mut self) -> Result<super::ProtocolStream>;

        /// Close the unerlying socket.
        async fn close(&mut self) -> Result<()>;

        /// Returns true if this connection is closed or is closing.
        fn is_closed(&self) -> bool;

        /// Creates a new independently owned handle to the underlying socket.
        fn clone(&self) -> TransportConnection;

        /// Returns the count of active stream.
        fn actives(&self) -> usize;
    }

    /// The [*stream muliplexing*](https://docs.libp2p.io/concepts/multiplex/overview/)
    /// instance created by [`accept`](DriverConnection::accept) or [`connect`](DriverConnection::connect)
    /// functions.
    pub trait DriverStream: Sync + Send + Unpin {
        /// Get the stream's uuid.
        fn id(&self) -> &str;
        /// Return the remote peer's public key.
        fn public_key(&self) -> &PublicKey;

        /// Returns the local address that this stream is bound to.
        fn local_addr(&self) -> &Multiaddr;

        /// Returns the remote address that this stream is connected to.
        fn peer_addr(&self) -> &Multiaddr;
        /// Attempt to read data via this stream.
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<Result<usize>>;

        /// Attempt to write data via this stream.
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize>>;

        /// Attempt to flush the write data.
        fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>>;

        /// Close this connection.
        fn poll_close(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>>;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverTransport`](transport_syscall::DriverTransport)"]
    Transport[transport_syscall::DriverTransport]
);

driver_wrapper!(
    ["A type wrapper of [`DriverListener`](transport_syscall::DriverListener)"]
    TransportListener[transport_syscall::DriverListener]
);

impl TransportListener {
    pub fn into_incoming(self) -> impl futures::Stream<Item = Result<TransportConnection>> + Unpin {
        Box::pin(unfold(self, |mut listener| async move {
            let res = listener.accept().await;
            Some((res, listener))
        }))
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverConnection`](transport_syscall::DriverConnection)"]
    TransportConnection[transport_syscall::DriverConnection]
);

impl TransportConnection {
    /// A wrapper of driver's [`close`](transport_syscall::DriverConnection::close) function.
    ///
    /// This function first removes self from [`Switch`] before calling the driver `close` function.
    pub async fn close(&mut self, switch: &Switch) {
        switch.remove_conn(self).await;
        _ = self.as_driver().close().await;
    }

    pub fn clone(&self) -> TransportConnection {
        self.0.clone()
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverStream`](transport_syscall::DriverStream)"]
    ProtocolStream[transport_syscall::DriverStream]
);

#[cfg(feature = "global_register")]
#[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
impl ProtocolStream {
    /// Connect to peer and negotiate a protocol. the `protos` is the list of candidate protocols.
    pub async fn connect<'a, C, E, I>(
        target: C,
        protos: I,
    ) -> crate::Result<(ProtocolStream, String)>
    where
        C: TryInto<crate::switch::ConnectTo<'a>, Error = E>,
        I: IntoIterator,
        I::Item: AsRef<str>,
        E: std::fmt::Debug,
    {
        Self::connect_with(crate::global_switch(), target, protos).await
    }
}

impl ProtocolStream {
    /// Connect to peer and negotiate a protocol. the `protos` is the list of candidate protocols.
    pub async fn connect_with<'a, C, E, I>(
        switch: &Switch,
        target: C,
        protos: I,
    ) -> crate::Result<(ProtocolStream, String)>
    where
        C: TryInto<crate::switch::ConnectTo<'a>, Error = E>,
        I: IntoIterator,
        I::Item: AsRef<str>,
        E: std::fmt::Debug,
    {
        switch.connect(target, protos).await
    }

    /// Send a ping request to target and check the response.
    pub async fn ping<'a, C, E>(switch: &Switch, target: C) -> crate::Result<()>
    where
        C: TryInto<crate::switch::ConnectTo<'a>, Error = E>,
        E: std::fmt::Debug,
    {
        let (mut stream, _) = Self::connect_with(switch, target, [PROTOCOL_IPFS_PING]).await?;

        let mut buf = vec![0u8; 32];

        thread_rng().fill_bytes(&mut buf);

        stream.write_all(&buf).await?;

        let mut echo = vec![0u8; 32];

        stream.read_exact(&mut echo).await?;

        if echo != buf {
            return Err(Error::Ping);
        }

        Ok(())
    }
}

impl AsyncWrite for ProtocolStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(self.as_driver()).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(self.as_driver()).poll_flush(cx)
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(self.as_driver()).poll_close(cx)
    }
}

impl AsyncRead for ProtocolStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(self.as_driver()).poll_read(cx, buf)
    }
}
