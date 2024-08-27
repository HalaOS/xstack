use std::{
    io::{self, Result},
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};

use async_trait::async_trait;
use futures::{AsyncRead, AsyncWrite};
use futures_boring::{
    ec, pkey,
    ssl::{SslAlert, SslContextBuilder, SslMethod, SslVerifyError, SslVerifyMode, SslVersion},
    x509::X509,
};
use futures_quic::{
    quiche::{self, Config},
    QuicConn, QuicConnect, QuicListener, QuicListenerBind, QuicStream,
};

use xstack::{
    identity::PublicKey,
    keystore::KeyStore,
    multiaddr::{Multiaddr, Protocol, ToSockAddr},
    transport::{
        syscall::{DriverConnection, DriverListener, DriverStream, DriverTransport},
        Listener, ProtocolStream, TransportConnection,
    },
    Switch,
};

async fn create_quic_config(host_key: &KeyStore, timeout: Duration) -> io::Result<Config> {
    let (cert, pk) = xstack_x509::generate(host_key).await?;

    let cert = X509::from_der(&cert)?;

    let pk = pkey::PKey::from_ec_key(ec::EcKey::private_key_from_der(&pk)?)?;

    let mut ssl_context_builder = SslContextBuilder::new(SslMethod::tls())?;

    ssl_context_builder.set_max_proto_version(Some(SslVersion::TLS1_3))?;
    ssl_context_builder.set_min_proto_version(Some(SslVersion::TLS1_3))?;

    ssl_context_builder.set_certificate(&cert)?;

    ssl_context_builder.set_private_key(&pk)?;

    ssl_context_builder.check_private_key()?;

    ssl_context_builder.set_custom_verify_callback(
        SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT,
        |ssl| {
            let cert = ssl
                .peer_certificate()
                .ok_or(SslVerifyError::Invalid(SslAlert::CERTIFICATE_REQUIRED))?;

            let cert = cert
                .to_der()
                .map_err(|_| SslVerifyError::Invalid(SslAlert::BAD_CERTIFICATE))?;

            let peer_id = xstack_x509::verify(cert)
                .map_err(|_| SslVerifyError::Invalid(SslAlert::BAD_CERTIFICATE))?
                .to_peer_id();

            log::trace!("ssl_server: verified peer={}", peer_id);

            Ok(())
        },
    );

    let mut config =
        Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, ssl_context_builder)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1024 * 1024);
    config.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);
    config.set_max_idle_timeout(timeout.as_millis() as u64);

    config.verify_peer(true);

    config.set_application_protos(&[b"libp2p"]).unwrap();

    // config.enable_early_data();

    config.set_disable_active_migration(false);

    Ok(config)
}

/// A libp2p transport backed quic protocol.
pub struct QuicTransport(pub Duration);

impl Default for QuicTransport {
    fn default() -> Self {
        Self(Duration::from_secs(2))
    }
}

#[async_trait]
impl DriverTransport for QuicTransport {
    async fn bind(&self, laddr: &Multiaddr, switch: Switch) -> Result<Listener> {
        let quic_config = create_quic_config(switch.keystore(), self.0).await?;

        let laddrs = laddr.to_sockaddr()?;

        let listener = QuicListener::bind(laddrs, quic_config).await?;

        let laddr = listener.local_addrs().next().unwrap().clone();

        Ok(QuicP2pListener::new(listener, laddr).into())
    }

    /// Connect to peer with remote peer [`raddr`](Multiaddr).
    async fn connect(&self, raddr: &Multiaddr, switch: Switch) -> Result<TransportConnection> {
        let mut quic_config = create_quic_config(switch.keystore(), self.0).await?;

        let raddr = raddr.to_sockaddr()?;

        let laddr = if raddr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };

        let conn = QuicConn::connect(None, laddr, raddr, &mut quic_config).await?;

        let (laddr, raddr) = conn
            .path()
            .await
            .ok_or(io::Error::new(io::ErrorKind::Other, "quic: no valid path"))?;

        let cert = conn.peer_cert().await.ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            "quic: peer cert not found",
        ))?;

        let public_key = xstack_x509::verify(cert)?;

        let conn = QuicP2pConn::new(laddr, raddr, conn, public_key);

        Ok(conn.into())
    }

    /// Check if this transport support the protocol stack represented by the `addr`.
    fn multiaddr_hit(&self, addr: &Multiaddr) -> bool {
        let stack = addr.protocol_stack().collect::<Vec<_>>();

        if stack.len() > 1 {
            if stack[1] == "udp" {
                return true;
            }
        }

        return false;
    }
}

struct QuicP2pListener {
    listener: QuicListener,
    laddr: SocketAddr,
}

impl QuicP2pListener {
    fn new(listener: QuicListener, laddr: SocketAddr) -> Self {
        Self { laddr, listener }
    }
}

#[async_trait]
impl DriverListener for QuicP2pListener {
    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> Result<TransportConnection> {
        let conn = self.listener.accept().await?;

        let cert = conn.peer_cert().await.ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            "quic: peer cert not found",
        ))?;

        let public_key = xstack_x509::verify(cert)?;

        let peer_addr = conn.peer_addr(self.laddr).await.ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            "quic: peer path not found",
        ))?;

        Ok(QuicP2pConn::new(self.laddr.clone(), peer_addr, conn, public_key).into())
    }

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> Result<Multiaddr> {
        let mut addr = Multiaddr::from(self.laddr.ip());
        addr.push(Protocol::Udp(self.laddr.port()));
        addr.push(Protocol::QuicV1);

        Ok(addr)
    }
}

#[derive(Clone)]
struct QuicP2pConn {
    laddr: Multiaddr,
    raddr: Multiaddr,
    conn: Arc<QuicConn>,
    public_key: PublicKey,
    is_closed: Arc<AtomicBool>,
    id: String,
    counter: Arc<AtomicUsize>,
}

impl QuicP2pConn {
    fn new(laddr: SocketAddr, raddr: SocketAddr, conn: QuicConn, public_key: PublicKey) -> Self {
        let mut m_laddr = Multiaddr::from(laddr.ip());
        m_laddr.push(Protocol::Udp(laddr.port()));
        m_laddr.push(Protocol::QuicV1);

        let mut m_raddr = Multiaddr::from(raddr.ip());
        m_raddr.push(Protocol::Udp(raddr.port()));
        m_raddr.push(Protocol::QuicV1);

        Self {
            id: format!("quic({:?})", conn.scid()),
            laddr: m_laddr,
            raddr: m_raddr,
            conn: Arc::new(conn),
            public_key,
            is_closed: Default::default(),
            counter: Default::default(),
        }
    }
}

#[async_trait]
impl DriverConnection for QuicP2pConn {
    fn id(&self) -> &str {
        &self.id
    }

    /// Returns local bind address.
    ///
    /// This can be useful, for example, when binding to port 0 to figure out which port was
    /// actually bound.
    fn local_addr(&self) -> &Multiaddr {
        &self.laddr
    }

    /// Returns the remote address that this connection is connected to.
    fn peer_addr(&self) -> &Multiaddr {
        &self.raddr
    }

    /// Accept newly incoming stream for reading/writing.
    ///
    /// If the connection is dropping or has been dropped, this function will returns `None`.
    async fn accept(&mut self) -> io::Result<ProtocolStream> {
        let stream = self.conn.accept().await?;

        Ok(QuicP2pStream::new(
            stream,
            self.public_key.clone(),
            self.laddr.clone(),
            self.raddr.clone(),
            self.counter.clone(),
        )
        .into())
    }

    async fn connect(&mut self) -> Result<ProtocolStream> {
        let stream = self.conn.open(true).await?;

        Ok(QuicP2pStream::new(
            stream,
            self.public_key.clone(),
            self.laddr.clone(),
            self.raddr.clone(),
            self.counter.clone(),
        )
        .into())
    }

    async fn close(&mut self) -> io::Result<()> {
        self.conn.close()?;

        self.is_closed.store(true, Ordering::Relaxed);

        Ok(())
    }

    /// Returns true if this connection is closed or is closing.
    fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::Relaxed)
    }

    /// Creates a new independently owned handle to the underlying socket.
    fn clone(&self) -> TransportConnection {
        Clone::clone(self).into()
    }

    /// Return the remote peer's public key.
    fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    fn actives(&self) -> usize {
        self.counter.load(Ordering::Relaxed)
    }
}

struct QuicP2pStream {
    id: String,
    stream: QuicStream,
    public_key: PublicKey,
    laddr: Multiaddr,
    raddr: Multiaddr,
    counter: Arc<AtomicUsize>,
}

impl Drop for QuicP2pStream {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

impl QuicP2pStream {
    fn new(
        stream: QuicStream,
        public_key: PublicKey,
        laddr: Multiaddr,
        raddr: Multiaddr,
        counter: Arc<AtomicUsize>,
    ) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);

        Self {
            counter,
            id: format!("quic({:?},{})", stream.scid(), stream.id()),
            stream,
            public_key,
            laddr,
            raddr,
        }
    }
}

#[async_trait]
impl DriverStream for QuicP2pStream {
    fn id(&self) -> &str {
        &self.id
    }
    /// Return the remote peer's public key.
    fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// Returns the local address that this stream is bound to.
    fn local_addr(&self) -> &Multiaddr {
        &self.laddr
    }

    /// Returns the remote address that this stream is connected to.
    fn peer_addr(&self) -> &Multiaddr {
        &self.raddr
    }
    /// Attempt to read data via this stream.
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }

    /// Attempt to write data via this stream.
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    /// Attempt to flush the write data.
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    /// Close this connection.
    fn poll_close(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.stream).poll_close(cx)
    }
}

#[cfg(test)]
mod tests {

    use async_trait::async_trait;
    use xstack::{Result, Switch};
    use xstack_spec::transport::{transport_specs, TransportSpecContext};

    use super::*;

    struct QuicMock;

    #[async_trait]
    impl TransportSpecContext for QuicMock {
        async fn create_switch(&self) -> Result<Switch> {
            let switch = Switch::new("test")
                .transport(QuicTransport::default())
                .transport_bind(["/ip4/127.0.0.1/udp/0/quic-v1"])
                .create()
                .await?;

            Ok(switch)
        }
    }

    #[futures_test::test]
    async fn test_specs() {
        // pretty_env_logger::init();
        transport_specs(QuicMock).await.unwrap();
    }
}
