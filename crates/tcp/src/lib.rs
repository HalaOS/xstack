//! A [***libp2p TCP transport protocol with TLS encryption***](https://docs.libp2p.io/concepts/secure-comm/tls/) implementation.

use std::io::Result;

use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};

use rasi::net::{TcpListener, TcpStream};

use xstack::multiaddr::{Multiaddr, Protocol, ToSockAddr};
use xstack::transport_syscall::DriverTransport;
use xstack::Switch;
use xstack::{TransportConnection, TransportListener};
use xstack_tls::{TlsConn, TlsListener};

/// The libp2p tcp transport implementation.
pub struct TcpTransport;

#[async_trait]
impl DriverTransport for TcpTransport {
    async fn bind(&self, switch: &Switch, laddr: &Multiaddr) -> Result<TransportListener> {
        let listener = TcpListener::bind(laddr.to_sockaddr()?).await?;

        let local_addr = listener.local_addr()?;

        let local_addr = Multiaddr::from(local_addr.ip())
            .with(Protocol::Tcp(local_addr.port()))
            .with(Protocol::Tls);

        let incoming = listener.into_stream().filter_map(|stream| async move {
            match stream {
                Ok(stream) => match stream.peer_addr() {
                    Ok(peer_addr) => {
                        let peer_addr = Multiaddr::from(peer_addr.ip())
                            .with(Protocol::Tcp(peer_addr.port()))
                            .with(Protocol::Tls);

                        return Some(Ok((stream, peer_addr)));
                    }
                    Err(_) => {
                        return None;
                    }
                },
                Err(err) => return Some(Err(err)),
            }
        });

        Ok(TlsListener::new(&switch, local_addr, Box::pin(incoming))
            .await?
            .into())
    }

    /// Connect to peer with remote peer [`raddr`](Multiaddr).
    async fn connect(&self, switch: &Switch, raddr: &Multiaddr) -> Result<TransportConnection> {
        let stream = TcpStream::connect(raddr.to_sockaddr()?).await?;

        let local_addr = stream.local_addr()?;

        let local_addr = Multiaddr::from(local_addr.ip())
            .with(Protocol::Tcp(local_addr.port()))
            .with(Protocol::Tls);

        let conn = TlsConn::connect(&switch, stream, local_addr, raddr.clone()).await?;

        Ok(conn.into())
    }

    /// Check if this transport support the protocol stack represented by the `addr`.
    fn multiaddr_hit(&self, addr: &Multiaddr) -> bool {
        for proto in addr.iter() {
            if proto == Protocol::P2pCircuit {
                return false;
            }
        }

        let stack = addr.protocol_stack().collect::<Vec<_>>();

        if stack.len() > 1 {
            if stack[1] == "tcp" {
                return true;
            }
        }

        return false;
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use xstack::{Result, Switch};
    use xstack_spec::transport::{transport_specs, TransportSpecContext};

    use super::*;

    struct TcpMock;

    #[async_trait]
    impl TransportSpecContext for TcpMock {
        async fn create_switch(&self) -> Result<Switch> {
            let switch = Switch::new("test")
                .transport(TcpTransport)
                .transport_bind(["/ip4/127.0.0.1/tcp/0"])
                .create()
                .await?;

            Ok(switch)
        }
    }

    #[futures_test::test]
    async fn test_specs() {
        // pretty_env_logger::init();
        transport_specs(TcpMock).await.unwrap();
    }
}
