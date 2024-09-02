use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use futures::{lock::Mutex, TryStreamExt};
use futures_map::KeyWaitMap;
use rasi::task::spawn_ok;
use xstack::{
    multiaddr::{Multiaddr, Protocol},
    transport_syscall::{DriverListener, DriverTransport},
    AutoNAT, ProtocolListener, ProtocolListenerState, ProtocolStream, Switch, TransportConnection,
    TransportListener,
};
use xstack_tls::{create_ssl_acceptor, SslAcceptor, TlsConn};

use crate::{CircuitV2Rpc, Result, PROTOCOL_CIRCUIT_RELAY_STOP};

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

        Ok(CircuitListener::new(switch, laddr.clone()).await?.into())
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

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Hash, Eq, Ord)]
enum CircuitEvent {
    Incoming,
}

#[derive(Default)]
struct RawCircuitListener {
    proto_listener: Option<ProtocolListenerState>,
    incoming_conn: VecDeque<TransportConnection>,
}

impl RawCircuitListener {
    fn next_incoming(&mut self) -> Option<TransportConnection> {
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

    fn inbound(&mut self, conn: TransportConnection) -> bool {
        if self.is_closed() {
            return false;
        }

        self.incoming_conn.push_back(conn);

        true
    }
}

#[derive(Clone)]
struct CircuitListenerState {
    local_addr: Multiaddr,
    switch: Switch,
    raw: Arc<Mutex<RawCircuitListener>>,
    event_map: Arc<KeyWaitMap<CircuitEvent, ()>>,
    ssl_acceptor: SslAcceptor,
}

// handlers for circuit/stop server.
impl CircuitListenerState {
    async fn circuit_stop_server_loop(self, listener: ProtocolListener) {
        if let Err(err) = self.circuit_stop_server_loop_prv(listener).await {
            log::error!("circuit_relay_stop_handler, err={}", err);
        } else {
            log::error!("circuit_relay_stop_handler stopped.");
        }

        self.close().await;
    }

    async fn circuit_stop_server_loop_prv(&self, listener: ProtocolListener) -> Result<()> {
        let mut incoming = listener.into_incoming();

        while let Some((stream, _)) = incoming.try_next().await? {
            if AutoNAT::Nat != self.switch.nat().await {
                log::trace!("drop inbound stream, the switch is not in the nat status.");
                self.raw.lock().await.pause();
                continue;
            }

            spawn_ok(self.clone().handle_circuit_stop_incoming_stream(stream));
        }

        Ok(())
    }

    async fn handle_circuit_stop_incoming_stream(self, mut stream: ProtocolStream) {
        if let Err(err) =
            CircuitV2Rpc::circuit_v2_stop_connect_accept(&mut stream, self.switch.max_packet_size())
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

        let conn = match TlsConn::accept(stream, local_addr, peer_addr.clone(), &self.ssl_acceptor)
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

        if self.switch.nat().await == AutoNAT::Nat {
            if self.raw.lock().await.inbound(conn.into()) {
                self.event_map.insert(CircuitEvent::Incoming, ());
            }
        }
    }
}

impl CircuitListenerState {
    async fn new(switch: Switch, local_addr: Multiaddr) -> Result<Self> {
        let listener = switch.bind([PROTOCOL_CIRCUIT_RELAY_STOP]).await?;

        let ssl_acceptor = create_ssl_acceptor(&switch).await?;

        let this = Self {
            switch,
            local_addr,
            ssl_acceptor,
            raw: Arc::new(Mutex::new(RawCircuitListener {
                proto_listener: Some(listener.to_state()),
                ..Default::default()
            })),
            event_map: Default::default(),
        };

        // start '/libp2p/circuit/relay/0.2.0/stop' server handle.
        spawn_ok(this.clone().circuit_stop_server_loop(listener));

        Ok(this)
    }
    async fn close(&self) {
        self.raw.lock().await.close().await;
        self.event_map.cancel_all();
    }

    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> std::io::Result<TransportConnection> {
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

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> std::io::Result<Multiaddr> {
        Ok(self.local_addr.clone())
    }
}

struct CircuitListener {
    state: CircuitListenerState,
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
    async fn new(switch: Switch, local_addr: Multiaddr) -> Result<Self> {
        Ok(Self {
            state: CircuitListenerState::new(switch, local_addr).await?,
        })
    }
}

#[async_trait]
impl DriverListener for CircuitListener {
    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> std::io::Result<TransportConnection> {
        self.state.accept().await
    }

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> std::io::Result<Multiaddr> {
        self.state.local_addr()
    }
}
