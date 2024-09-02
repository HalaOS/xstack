use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use xstack::{
    multiaddr::{Multiaddr, Protocol},
    transport_syscall::DriverTransport,
    Switch, TransportConnection, TransportListener,
};

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

        todo!()
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
