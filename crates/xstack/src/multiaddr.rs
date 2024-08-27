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
