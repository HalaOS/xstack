//! Implementation of [multiaddr](https://github.com/multiformats/multiaddr) in Rust.

use std::net::{IpAddr, SocketAddr};

pub use multiaddr::*;

/// A trait convert object to [`SocketAddr`]
pub trait ToSockAddr {
    type Error;
    fn to_sockaddr(&self) -> std::result::Result<SocketAddr, Self::Error>;
}

impl ToSockAddr for Multiaddr {
    type Error = crate::Error;

    fn to_sockaddr(&self) -> std::result::Result<SocketAddr, Self::Error> {
        let mut iter = self.iter();

        let ip = match iter
            .next()
            .ok_or(Error::UnknownProtocolString(self.to_string()))?
        {
            Protocol::Ip4(ip) => IpAddr::from(ip),
            Protocol::Ip6(ip) => IpAddr::from(ip),
            _ => return Err(Error::UnknownProtocolString(self.to_string()).into()),
        };

        let next = iter
            .next()
            .ok_or(Error::UnknownProtocolString(self.to_string()))?;

        match next {
            Protocol::Tcp(port) | Protocol::Udp(port) => {
                return Ok(SocketAddr::new(ip, port));
            }
            _ => return Err(Error::UnknownProtocolString(self.to_string()).into()),
        }
    }
}

/// Returns true if this [`Multiaddr`] is supported by `tcp/tls` transport.
pub fn is_tcp_transport(addr: &Multiaddr) -> bool {
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

    false
}

/// Returns true if this [`Multiaddr`] is supported by `quic` transport.
pub fn is_quic_transport(addr: &Multiaddr) -> bool {
    for proto in addr.iter() {
        if proto == Protocol::P2pCircuit {
            return false;
        }
    }

    let stack = addr.protocol_stack().collect::<Vec<_>>();

    if stack.len() > 2 {
        if stack[1] == "udp" && stack[2] == "quic-v1" {
            return true;
        }
    }

    false
}
