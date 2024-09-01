use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use async_trait::async_trait;
use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    AsyncRead, AsyncReadExt, AsyncWrite, SinkExt, StreamExt,
};

use futures_boring::{
    accept, ec, pkey,
    ssl::{SslAcceptor, SslAlert, SslMethod, SslVerifyError, SslVerifyMode, SslVersion},
    x509::X509,
};
use futures_yamux::{Reason, YamuxConn, YamuxStream, INIT_WINDOW_SIZE};
use multistream_select::listener_select_proto;
use rasi::{task::spawn_ok, timer::TimeoutExt};
use uuid::Uuid;
use xstack::{
    identity::PublicKey,
    multiaddr::{Multiaddr, Protocol},
    transport_syscall::{DriverConnection, DriverListener, DriverStream, DriverTransport},
    AutoNAT, ProtocolListener, ProtocolStream, Switch, TransportConnection, TransportListener,
};

use crate::{CircuitV2Rpc, Result, PROTOCOL_CIRCUIT_RELAY_HOP};

/// The implementation of transport [**circuit_v2**].
///
/// The below codes show how to enable this.
///
/// ```no_run
/// use xstack::Switch;
/// use xstack_circuit::CircuitTransport;
///
/// # async fn boostrap() {
/// Switch::new("test")
///       .transport(CircuitTransport)
///       // if the node want to enable sub-protocol '/libp2p/circuit/relay/0.2.0/stop'
///       .transport_bind(["/p2p-circuit"])
///       .create()
///       .await
///       .unwrap()
///       // register to global context.
///       .into_global();
///
/// # }
/// ```
///
/// Note: **the multiaddr '/p2p-circuit' can't be bound twice for same `Switch`.**
///
/// [**circuit_v2**]: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md#hop-protocol
///
#[derive(Default)]
pub struct CircuitTransport(AtomicBool);

#[allow(unused)]
#[async_trait]
impl DriverTransport for CircuitTransport {
    /// Create a server-side socket with provided [`laddr`](Multiaddr).
    async fn bind(&self, laddr: &Multiaddr, switch: Switch) -> std::io::Result<TransportListener> {
        if self
            .0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("Can't bind to {}", laddr),
            ));
        }

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

        let ssl_acceptor = ssl_acceptor_builder.build();

        Ok(CircuitListener::new(switch, ssl_acceptor).await?.into())
    }

    /// Connect to peer with remote peer [`raddr`](Multiaddr).
    async fn connect(
        &self,
        raddr: &Multiaddr,
        switch: Switch,
    ) -> std::io::Result<TransportConnection> {
        todo!()
    }

    /// Check if this transport support the protocol stack represented by the `addr`.
    fn multiaddr_hit(&self, addr: &Multiaddr) -> bool {
        let circuit_addr = Multiaddr::empty().with(Protocol::P2pCircuit);

        // for bind function.
        if *addr == circuit_addr {
            return true;
        }

        // below codes for connect function
        if addr.ends_with(&circuit_addr) {
            return true;
        }

        let mut addr = addr.clone();

        if let Some(Protocol::P2p(_)) = addr.pop() {
            if addr.ends_with(&circuit_addr) {
                return true;
            }
        }

        return false;
    }
}

struct CircuitListenerBackground {
    switch: Switch,
    sender: Sender<TransportConnection>,
    protocol_listener: ProtocolListener,
    ssl_acceptor: SslAcceptor,
}

impl CircuitListenerBackground {
    async fn incoming_loop(self) {
        while !self.sender.is_closed() {
            let stream = match self.protocol_listener.accept().await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    log::error!("circuit accept, err={}", err);
                    break;
                }
            };

            let timeout = self.switch.timeout();

            let raddr = stream.peer_addr().clone();

            let max_packet_size = self.switch.max_packet_size();

            let mut sender = self.sender.clone();

            let ssl_acceptor = self.ssl_acceptor.clone();

            spawn_ok(async move {
                match Self::accept(stream, ssl_acceptor, max_packet_size)
                    .timeout(timeout)
                    .await
                {
                    Some(Ok(conn)) => {
                        _ = sender.send(conn).await;
                    }
                    Some(Err(err)) => {
                        log::error!("inbound {}, err={}", raddr, err);
                    }
                    _ => {
                        log::error!("inbound {}, err=timeout", raddr);
                    }
                }
            });
        }
    }

    async fn accept(
        mut stream: ProtocolStream,
        ssl_acceptor: SslAcceptor,
        max_packet_size: usize,
    ) -> std::io::Result<TransportConnection> {
        let _limits =
            CircuitV2Rpc::circuit_v2_stop_connect_accept(&mut stream, max_packet_size).await?;

        let (_, _) = listener_select_proto(&mut stream, ["/tls/1.0.0"]).await?;

        let local_addr = stream.local_addr().clone();
        let peer_addr = stream.peer_addr().clone();

        let mut stream = accept(&ssl_acceptor, stream).await.map_err(|err| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, format!("{}", err))
        })?;

        let cert = stream.ssl().peer_certificate().ok_or(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Handshaking",
        ))?;

        let public_key = xstack_x509::verify(cert.to_der()?)?;

        let (_, _) = listener_select_proto(&mut stream, ["/yamux/1.0.0"]).await?;

        let conn = P2pTcpConn::new(local_addr, peer_addr, public_key, stream, true)?;

        Ok(conn.into())
    }
}

struct CircuitListener {
    switch: Switch,
    receiver: Receiver<TransportConnection>,
}

impl CircuitListener {
    async fn new(switch: Switch, ssl_acceptor: SslAcceptor) -> Result<Self> {
        let protocol_listener = switch.bind([PROTOCOL_CIRCUIT_RELAY_HOP]).await?;

        let (sender, receiver) = channel(100);

        let background = CircuitListenerBackground {
            switch: switch.clone(),
            protocol_listener,
            sender,
            ssl_acceptor,
        };

        spawn_ok(background.incoming_loop());

        Ok(Self { receiver, switch })
    }
}

#[async_trait]
impl DriverListener for CircuitListener {
    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> std::io::Result<TransportConnection> {
        while let Some(conn) = self.receiver.next().await {
            if AutoNAT::Nat == self.switch.nat().await {
                return Ok(conn);
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "circuit/stop, broken".to_owned(),
        ))
    }

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> std::io::Result<Multiaddr> {
        Ok(Multiaddr::empty().with(Protocol::P2pCircuit))
    }
}

#[derive(Clone)]
struct P2pTcpConn {
    public_key: PublicKey,
    laddr: Multiaddr,
    raddr: Multiaddr,
    conn: Arc<YamuxConn>,
    id: String,
    counter: Arc<AtomicUsize>,
}

impl P2pTcpConn {
    fn new<S>(
        laddr: Multiaddr,
        raddr: Multiaddr,
        public_key: PublicKey,
        stream: S,
        is_server: bool,
    ) -> std::io::Result<Self>
    where
        S: AsyncWrite + AsyncRead + 'static + Sync + Send,
    {
        let (read, write) = stream.split();
        let conn = futures_yamux::YamuxConn::new_with(INIT_WINDOW_SIZE, is_server, read, write);

        Ok(Self {
            laddr,
            raddr,
            conn: Arc::new(conn),
            public_key,
            id: Uuid::new_v4().to_string(),
            counter: Default::default(),
        })
    }
}

#[async_trait]
impl DriverConnection for P2pTcpConn {
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
    async fn accept(&mut self) -> std::io::Result<ProtocolStream> {
        let stream = self.conn.stream_accept().await?;

        Ok(P2pTcpStream::new(
            self.id.clone(),
            stream,
            self.public_key.clone(),
            self.laddr.clone(),
            self.raddr.clone(),
            self.counter.clone(),
        )
        .into())
    }

    async fn connect(&mut self) -> std::io::Result<ProtocolStream> {
        let stream = self.conn.stream_open().await?;

        Ok(P2pTcpStream::new(
            self.id.clone(),
            stream,
            self.public_key.clone(),
            self.laddr.clone(),
            self.raddr.clone(),
            self.counter.clone(),
        )
        .into())
    }

    async fn close(&mut self) -> std::io::Result<()> {
        self.conn.close(Reason::Normal)?;

        Ok(())
    }

    /// Returns true if this connection is closed or is closing.
    fn is_closed(&self) -> bool {
        self.conn.is_closed()
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

struct P2pTcpStream {
    conn_id: String,
    id: String,
    stream: YamuxStream,
    public_key: PublicKey,
    laddr: Multiaddr,
    raddr: Multiaddr,
    counter: Arc<AtomicUsize>,
}

impl Drop for P2pTcpStream {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

impl P2pTcpStream {
    fn new(
        conn_id: String,
        stream: YamuxStream,
        public_key: PublicKey,
        laddr: Multiaddr,
        raddr: Multiaddr,
        counter: Arc<AtomicUsize>,
    ) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);

        Self {
            conn_id,
            counter,
            id: Uuid::new_v4().to_string(),
            stream,
            public_key,
            laddr,
            raddr,
        }
    }
}

#[async_trait]
impl DriverStream for P2pTcpStream {
    fn conn_id(&self) -> &str {
        &self.conn_id
    }
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
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }

    /// Attempt to write data via this stream.
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    /// Attempt to flush the write data.
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    /// Close this connection.
    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_close(cx)
    }
}
