use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;

use xstack::{
    multiaddr::{Multiaddr, Protocol},
    transport_syscall::DriverTransport,
    P2pConn, Switch, TransportListener,
};
use xstack_tls::TlsConn;

use crate::{CircuitV2Rpc, Error, PROTOCOL_CIRCUIT_RELAY_HOP};

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
        panic!("Check multiaddr_hit fn.");
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

        // // for bind function.
        // if *addr == circuit_addr {
        //     return true;
        // }

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
