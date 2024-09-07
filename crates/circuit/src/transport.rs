use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use futures::{lock::Mutex, StreamExt, TryStreamExt};
use futures_map::KeyWaitMap;
use rasi::task::spawn_ok;
use xstack::{
    events,
    multiaddr::{Multiaddr, Protocol},
    transport_syscall::{DriverListener, DriverTransport},
    AutoNAT, EventSource, P2pConn, ProtocolListener, ProtocolListenerCloser, ProtocolStream,
    Switch, TransportListener,
};
use xstack_tls::{create_ssl_acceptor, SslAcceptor, TlsConn};

use crate::{CircuitV2Rpc, Error, Result, PROTOCOL_CIRCUIT_RELAY_HOP, PROTOCOL_CIRCUIT_RELAY_STOP};

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
///       .transport(CircuitTransport::default())
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
pub struct CircuitTransport {
    reentrancy_guard: AtomicBool,
    activities: Arc<AtomicUsize>,
}

#[allow(unused)]
#[async_trait]
impl DriverTransport for CircuitTransport {
    fn name(&self) -> &str {
        "circuit"
    }
    fn activities(&self) -> usize {
        self.activities.load(Ordering::Relaxed)
    }
    /// Create a server-side socket with provided [`laddr`](Multiaddr).
    async fn bind(&self, switch: &Switch, laddr: &Multiaddr) -> std::io::Result<TransportListener> {
        if self
            .reentrancy_guard
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("Can't bind to {}", laddr),
            ));
        }

        Ok(
            CircuitListener::new(&switch, laddr.clone(), self.activities.clone())
                .await?
                .into(),
        )
    }

    /// Connect to peer with remote peer [`raddr`](Multiaddr).
    async fn connect(&self, switch: &Switch, raddr: &Multiaddr) -> std::io::Result<P2pConn> {
        let peer_addr = raddr.clone();
        let mut raddr = raddr.clone();

        let circuit_addr = Multiaddr::empty().with(Protocol::P2pCircuit);

        let (raddr, peer_id) = if let Some(Protocol::P2p(id)) = raddr.pop() {
            if let Some(Protocol::P2pCircuit) = raddr.pop() {
                (raddr, id)
            } else {
                return Err(Error::ConnectAddr.into());
            }
        } else {
            return Err(Error::ConnectAddr.into());
        };

        log::trace!("circuit_v2, connect to hop={:?}", raddr);

        let (mut stream, _) = switch.connect(&raddr, [PROTOCOL_CIRCUIT_RELAY_HOP]).await?;

        let limits =
            CircuitV2Rpc::circuit_v2_hop_connect(&mut stream, &peer_id, switch.max_packet_size)
                .await?;

        log::trace!("circuit_v2, connection limits={:?}", limits);

        let local_addr = stream.local_addr().clone();

        let conn = TlsConn::connect(
            &switch,
            stream,
            local_addr,
            peer_addr,
            self.activities.clone(),
        )
        .await?;

        log::trace!("circuit_v2, connection handshaked");

        Ok(conn.into())
    }

    /// Check if this transport support the protocol stack represented by the `addr`.
    fn multiaddr_hit(&self, addr: &Multiaddr) -> bool {
        let circuit_addr = Multiaddr::empty().with(Protocol::P2pCircuit);

        // for bind function.
        if *addr == circuit_addr {
            return true;
        }

        // below codes for connect function

        let mut addr = addr.clone();

        if let Some(Protocol::P2p(_)) = addr.pop() {
            if addr.ends_with(&circuit_addr) {
                return true;
            }
        }

        return false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Hash, Eq, Ord)]
enum CircuitEvent {
    Incoming,
}

#[derive(Default)]
struct RawCircuitTransportState {
    proto_listener: Option<ProtocolListenerCloser>,
    incoming_conn: VecDeque<P2pConn>,
}

impl RawCircuitTransportState {
    fn next_incoming(&mut self) -> Option<P2pConn> {
        self.incoming_conn.pop_front()
    }

    fn is_closed(&self) -> bool {
        self.proto_listener.is_none()
    }

    async fn close(&mut self) {
        if let Some(listener) = self.proto_listener.take() {
            listener.close().await;
            self.incoming_conn.clear();
        }
    }

    fn pause(&mut self) {
        self.incoming_conn.clear();
    }

    fn inbound(&mut self, conn: P2pConn) -> bool {
        if self.is_closed() {
            return false;
        }

        self.incoming_conn.push_back(conn);

        true
    }
}

#[derive(Clone)]
struct CircuitTransportState {
    switch: Switch,
    raw: Arc<Mutex<RawCircuitTransportState>>,
    event_map: Arc<KeyWaitMap<CircuitEvent, ()>>,
}

impl Deref for CircuitTransportState {
    type Target = Switch;

    fn deref(&self) -> &Self::Target {
        &self.switch
    }
}

impl CircuitTransportState {
    async fn close(&self) {
        self.raw.lock().await.close().await;
        self.event_map.cancel_all();
    }

    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> std::io::Result<P2pConn> {
        loop {
            let mut raw = self.raw.lock().await;

            if raw.is_closed() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "CircuitListener is closed.",
                ));
            }

            if let Some(conn) = raw.next_incoming() {
                return Ok(conn);
            }

            self.event_map.wait(&CircuitEvent::Incoming, raw).await;
        }
    }
}

#[allow(unused)]
struct CircuitHopClient {
    event_source: EventSource<events::HandshakeSuccess>,
    state: CircuitTransportState,
}

impl CircuitHopClient {
    async fn bind(switch: &Switch, state: &CircuitTransportState) {
        let event_source = EventSource::bind_with(switch, NonZeroUsize::new(100).unwrap()).await;

        spawn_ok(
            Self {
                state: state.clone(),
                event_source,
            }
            .run_loop(),
        );
    }

    async fn run_loop(mut self) {
        if let Err(err) = self.run_loop_prv().await {
            log::error!("stop_server, stopped with error: {}", err);
        } else {
            log::error!("stop_server, stopped.");
        }

        self.state.close().await;
    }

    async fn run_loop_prv(&mut self) -> Result<()> {
        while let Some((_, peer_id)) = self.event_source.next().await {
            if let Some(peer_info) = self.state.lookup_peer_info(&peer_id).await? {
                if peer_info
                    .protos
                    .iter()
                    .find(|proto| proto.as_str() == PROTOCOL_CIRCUIT_RELAY_HOP)
                    .is_some()
                {
                    // log::trace!("found circuit_v2/hop node, {}", peer_id);
                }
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
struct CircuitStopServer {
    ssl_acceptor: SslAcceptor,
    state: CircuitTransportState,
    actives: Arc<AtomicUsize>,
}

impl CircuitStopServer {
    async fn bind(switch: &Switch, actives: Arc<AtomicUsize>) -> Result<CircuitTransportState> {
        let listener = switch.bind([PROTOCOL_CIRCUIT_RELAY_STOP]).await?;

        let ssl_acceptor = create_ssl_acceptor(&switch).await?;

        let state = CircuitTransportState {
            switch: switch.clone(),
            raw: Arc::new(Mutex::new(RawCircuitTransportState {
                proto_listener: Some(listener.to_closer()),
                ..Default::default()
            })),
            event_map: Default::default(),
        };

        spawn_ok(
            Self {
                ssl_acceptor,
                state: state.clone(),
                actives,
            }
            .run_loop(listener),
        );

        Ok(state)
    }

    async fn run_loop(self, listener: ProtocolListener) {
        if let Err(err) = self.circuit_stop_server_loop_prv(listener).await {
            log::error!("circuit_relay_stop_handler, err={}", err);
        } else {
            log::error!("circuit_relay_stop_handler stopped.");
        }

        self.state.close().await;
    }

    async fn circuit_stop_server_loop_prv(&self, listener: ProtocolListener) -> Result<()> {
        let mut incoming = listener.into_incoming();

        while let Some((stream, _)) = incoming.try_next().await? {
            if AutoNAT::NAT != self.state.switch.nat().await {
                log::trace!("drop inbound stream, the switch is not in the nat status.");
                self.state.raw.lock().await.pause();
                continue;
            }

            spawn_ok(self.clone().handle_circuit_stop_incoming_stream(stream));
        }

        Ok(())
    }

    async fn handle_circuit_stop_incoming_stream(self, mut stream: ProtocolStream) {
        if let Err(err) = CircuitV2Rpc::circuit_v2_stop_connect_accept(
            &mut stream,
            self.state.switch.max_packet_size,
        )
        .await
        {
            log::error!(
                "circuit_v2_stop_connect_accept, from={}, err={}",
                stream.peer_addr(),
                err
            );

            return;
        }

        let local_addr = stream.local_addr().clone();
        let peer_addr = stream.peer_addr().clone();

        let conn = match TlsConn::accept(
            stream,
            local_addr,
            peer_addr.clone(),
            &self.ssl_acceptor,
            self.actives.clone(),
        )
        .await
        {
            Ok(conn) => conn,
            Err(err) => {
                log::error!(
                    "circuit_v2_stop_connect_accept, from={}, err={}",
                    peer_addr,
                    err
                );
                return;
            }
        };

        if self.state.switch.nat().await == AutoNAT::NAT {
            if self.state.raw.lock().await.inbound(conn.into()) {
                self.state.event_map.insert(CircuitEvent::Incoming, ());
            }
        }
    }
}

struct CircuitListener {
    local_addr: Multiaddr,
    state: CircuitTransportState,
}

impl Drop for CircuitListener {
    fn drop(&mut self) {
        let state = self.state.clone();

        spawn_ok(async move {
            state.close().await;
        });
    }
}

impl CircuitListener {
    async fn new(
        switch: &Switch,
        local_addr: Multiaddr,
        actives: Arc<AtomicUsize>,
    ) -> Result<Self> {
        let state = CircuitStopServer::bind(switch, actives).await?;

        CircuitHopClient::bind(&switch, &state).await;

        Ok(Self { state, local_addr })
    }
}

#[async_trait]
impl DriverListener for CircuitListener {
    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> std::io::Result<P2pConn> {
        self.state.accept().await
    }

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> std::io::Result<Multiaddr> {
        Ok(self.local_addr.clone())
    }
}
