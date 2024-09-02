//! The [***secure channel***] implementation for xstack.
//!
//! [***secure channel***]: https://docs.libp2p.io/concepts/secure-comm/overview/

use std::{
    io::{Error, ErrorKind, Result},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use async_trait::async_trait;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, Stream, TryStreamExt};
use futures_boring::{
    accept, connect, ec, pkey,
    ssl::{SslAlert, SslConnector, SslMethod, SslVerifyError, SslVerifyMode, SslVersion},
    x509::X509,
};
use futures_yamux::{Reason, YamuxConn, YamuxStream, INIT_WINDOW_SIZE};
use multistream_select::{dialer_select_proto, listener_select_proto, Version};
use uuid::Uuid;
use xstack::{
    identity::PublicKey,
    multiaddr::{Multiaddr, ToSockAddr},
    transport_syscall::{DriverConnection, DriverListener, DriverStream},
    ProtocolStream, Switch, TransportConnection,
};

pub use futures_boring::ssl::SslAcceptor;
pub async fn create_ssl_acceptor(switch: &Switch) -> Result<SslAcceptor> {
    let (cert, pk) = xstack_x509::generate(switch.keystore()).await?;

    let cert = X509::from_der(&cert)?;

    let pk = pkey::PKey::from_ec_key(ec::EcKey::private_key_from_der(&pk)?)?;

    let mut ssl_acceptor_builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())?;

    ssl_acceptor_builder.set_max_proto_version(Some(SslVersion::TLS1_3))?;
    ssl_acceptor_builder.set_min_proto_version(Some(SslVersion::TLS1_3))?;

    ssl_acceptor_builder.set_certificate(&cert)?;

    ssl_acceptor_builder.set_private_key(&pk)?;

    ssl_acceptor_builder.check_private_key()?;

    ssl_acceptor_builder.set_custom_verify_callback(
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

    Ok(ssl_acceptor_builder.build())
}

/// A listener over tls/yamux.
pub struct TlsListener<Incoming> {
    /// the multiaddr to which the listener is bound.
    local_addr: Multiaddr,
    /// underlying listener.
    incoming: Incoming,
    /// ssl acceptor from boring crate.
    ssl_acceptor: SslAcceptor,
}

impl<Incoming> TlsListener<Incoming> {
    /// Create a new `TlsListener` instance.
    pub async fn new(switch: &Switch, local_addr: Multiaddr, incoming: Incoming) -> Result<Self> {
        let ssl_acceptor = create_ssl_acceptor(switch).await?;

        Ok(Self {
            ssl_acceptor,
            incoming,
            local_addr,
        })
    }
}

#[async_trait]
impl<Incoming, S> DriverListener for TlsListener<Incoming>
where
    Incoming: Stream<Item = Result<(S, Multiaddr)>> + Sync + Send + Unpin,
    S: AsyncWrite + AsyncRead + Sync + Send + Unpin + 'static,
{
    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> Result<TransportConnection> {
        let (stream, raddr) = match self.incoming.try_next().await? {
            Some((stream, raddr)) => (stream, raddr),
            None => {
                return Err(Error::new(ErrorKind::BrokenPipe, "TlsListener closed"));
            }
        };

        Ok(
            TlsConn::accept(stream, self.local_addr.clone(), raddr, &self.ssl_acceptor)
                .await?
                .into(),
        )
    }

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> Result<Multiaddr> {
        Ok(self.local_addr.clone())
    }
}

/// A connection over tls/yamux.
#[derive(Clone)]
pub struct TlsConn {
    id: String,
    public_key: PublicKey,
    local_addr: Multiaddr,
    peer_addr: Multiaddr,
    conn: Arc<YamuxConn>,
    stream_count: Arc<AtomicUsize>,
}

impl TlsConn {
    /// Create a client-side `TlsConn` instance.
    pub async fn connect<S>(
        switch: &Switch,
        mut stream: S,
        local_addr: Multiaddr,
        peer_addr: Multiaddr,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Sync + Send + Unpin + 'static,
    {
        let (cert, pk) = xstack_x509::generate(switch.keystore()).await?;

        let cert = X509::from_der(&cert)?;

        let pk = pkey::PKey::from_ec_key(ec::EcKey::private_key_from_der(&pk)?)?;

        let mut config = SslConnector::builder(SslMethod::tls_client())?;

        config.set_certificate(&cert)?;

        config.set_private_key(&pk)?;

        config.set_max_proto_version(Some(SslVersion::TLS1_3))?;
        config.set_min_proto_version(Some(SslVersion::TLS1_3))?;

        config.set_custom_verify_callback(SslVerifyMode::PEER, |ssl| {
            let cert = ssl
                .peer_certificate()
                .ok_or(SslVerifyError::Invalid(SslAlert::CERTIFICATE_REQUIRED))?;

            let cert = cert.to_der().map_err(|err| {
                log::error!("{}", err);
                SslVerifyError::Invalid(SslAlert::BAD_CERTIFICATE)
            })?;

            let peer_id = xstack_x509::verify(cert)
                .map_err(|err| {
                    log::error!("{}", err);
                    SslVerifyError::Invalid(SslAlert::BAD_CERTIFICATE)
                })?
                .to_peer_id();

            log::trace!("ssl_client: verified peer={}", peer_id);

            Ok(())
        });

        let config = config.build().configure()?;

        // dynamic select the secure protocol.
        let (_, _) = dialer_select_proto(&mut stream, ["/tls/1.0.0"], Version::V1).await?;

        let server_name = peer_addr.to_sockaddr()?.ip().to_string();

        let mut stream = connect(config, &server_name, stream)
            .await
            .map_err(|err| Error::new(ErrorKind::BrokenPipe, err.to_string()))?;

        let cert = stream
            .ssl()
            .peer_certificate()
            .ok_or(Error::new(ErrorKind::Other, "Handshaking"))?;

        let public_key = xstack_x509::verify(cert.to_der()?)?;

        let (_, _) = dialer_select_proto(&mut stream, ["/yamux/1.0.0"], Version::V1).await?;

        let conn = TlsConn::new(stream, false, public_key, local_addr, peer_addr)?;

        Ok(conn.into())
    }

    pub async fn accept<S>(
        mut stream: S,
        local_addr: Multiaddr,
        peer_addr: Multiaddr,
        acceptor: &SslAcceptor,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Sync + Send + Unpin + 'static,
    {
        let (_, _) = listener_select_proto(&mut stream, ["/tls/1.0.0"]).await?;

        let mut stream = accept(acceptor, stream)
            .await
            .map_err(|err| Error::new(ErrorKind::BrokenPipe, err.to_string()))?;

        let cert = stream
            .ssl()
            .peer_certificate()
            .ok_or(Error::new(ErrorKind::Other, "Handshaking"))?;

        let public_key = xstack_x509::verify(cert.to_der()?)?;

        let (_, _) = listener_select_proto(&mut stream, ["/yamux/1.0.0"]).await?;

        let conn = TlsConn::new(stream, true, public_key, local_addr, peer_addr)?;

        Ok(conn.into())
    }

    fn new<S>(
        stream: S,
        is_server: bool,
        public_key: PublicKey,
        local_addr: Multiaddr,
        peer_addr: Multiaddr,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Sync + Send + 'static,
    {
        let (read, write) = stream.split();
        let conn = futures_yamux::YamuxConn::new_with(INIT_WINDOW_SIZE, is_server, read, write);

        Ok(Self {
            local_addr,
            peer_addr,
            conn: Arc::new(conn),
            public_key,
            id: Uuid::new_v4().to_string(),
            stream_count: Default::default(),
        })
    }
}

#[async_trait]
impl DriverConnection for TlsConn {
    fn id(&self) -> &str {
        &self.id
    }

    fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    fn local_addr(&self) -> &Multiaddr {
        &self.local_addr
    }

    fn peer_addr(&self) -> &Multiaddr {
        &self.peer_addr
    }

    fn is_closed(&self) -> bool {
        self.conn.is_closed()
    }

    fn actives(&self) -> usize {
        self.stream_count.load(Ordering::Relaxed)
    }

    fn clone(&self) -> TransportConnection {
        Clone::clone(self).into()
    }

    async fn accept(&mut self) -> Result<ProtocolStream> {
        let stream = self.conn.stream_accept().await?;

        Ok(TlsStream::new(
            self.id.clone(),
            self.public_key.clone(),
            self.local_addr.clone(),
            self.peer_addr.clone(),
            self.stream_count.clone(),
            stream,
        )
        .into())
    }

    async fn connect(&mut self) -> Result<ProtocolStream> {
        let stream = self.conn.stream_open().await?;

        Ok(TlsStream::new(
            self.id.clone(),
            self.public_key.clone(),
            self.local_addr.clone(),
            self.peer_addr.clone(),
            self.stream_count.clone(),
            stream,
        )
        .into())
    }

    async fn close(&mut self) -> Result<()> {
        self.conn.close(Reason::Normal)?;

        Ok(())
    }
}

/// A stream over tls/yamux.
pub struct TlsStream {
    id: String,
    conn_id: String,
    public_key: PublicKey,
    local_addr: Multiaddr,
    peer_addr: Multiaddr,
    stream_counter: Arc<AtomicUsize>,
    stream: YamuxStream,
}

impl Drop for TlsStream {
    fn drop(&mut self) {
        self.stream_counter.fetch_sub(1, Ordering::Relaxed);
    }
}

impl TlsStream {
    fn new(
        conn_id: String,
        public_key: PublicKey,
        local_addr: Multiaddr,
        peer_addr: Multiaddr,
        stream_counter: Arc<AtomicUsize>,
        stream: YamuxStream,
    ) -> Self {
        stream_counter.fetch_add(1, Ordering::Relaxed);

        Self {
            conn_id,
            stream_counter,
            id: Uuid::new_v4().to_string(),
            stream,
            public_key,
            local_addr,
            peer_addr,
        }
    }
}

#[async_trait]
impl DriverStream for TlsStream {
    fn conn_id(&self) -> &str {
        &self.conn_id
    }
    fn id(&self) -> &str {
        &self.id
    }

    fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// Returns the local address that this stream is bound to.
    fn local_addr(&self) -> &Multiaddr {
        &self.local_addr
    }

    /// Returns the remote address that this stream is connected to.
    fn peer_addr(&self) -> &Multiaddr {
        &self.peer_addr
    }
    /// Attempt to read data via this stream.
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }

    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_close(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.stream).poll_close(cx)
    }
}
