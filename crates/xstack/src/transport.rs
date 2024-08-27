//! A plugin system for libp2p's transport layer protocol.

use std::{io::Result, pin::Pin};

use futures::{stream::unfold, AsyncRead, AsyncWrite};

use crate::{driver_wrapper, switch::Switch};

/// A libp2p transport driver must implement the `Driver-*` traits in this module.
pub mod syscall {
    use std::{
        io::Result,
        task::{Context, Poll},
    };

    use async_trait::async_trait;
    use identity::PublicKey;
    use multiaddr::Multiaddr;

    use crate::switch::Switch;

    use super::*;

    /// A libp2p transport provider must implement this trait as the transport's main entry type.
    #[async_trait]
    pub trait DriverTransport: Send + Sync {
        /// Create a server-side socket with provided [`laddr`](Multiaddr).
        async fn bind(&self, laddr: &Multiaddr, switch: Switch) -> Result<Listener>;

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
    ["A type wrapper of [`DriverTransport`](syscall::DriverTransport)"]
    Transport[syscall::DriverTransport]
);

driver_wrapper!(
    ["A type wrapper of [`DriverListener`](syscall::DriverListener)"]
    Listener[syscall::DriverListener]
);

impl Listener {
    pub fn into_incoming(self) -> impl futures::Stream<Item = Result<TransportConnection>> + Unpin {
        Box::pin(unfold(self, |mut listener| async move {
            let res = listener.accept().await;
            Some((res, listener))
        }))
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverConnection`](syscall::DriverConnection)"]
    TransportConnection[syscall::DriverConnection]
);

impl TransportConnection {
    /// A wrapper of driver's [`close`](syscall::DriverConnection::close) function.
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
    ["A type wrapper of [`DriverStream`](syscall::DriverStream)"]
    ProtocolStream[syscall::DriverStream]
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
