//! A decorator pattern implementation that adds [**IP and Name Resolution**] capabilities to other transports.
//!
//!
//! Most libp2p transports use the IP protocol as a foundational layer, and as a result, most transport multiaddrs will begin with a component that represents an IPv4 or IPv6 address.
//!
//! This may be an actual address, such as /ip4/198.51.100 or /ip6/fe80::883:a581:fff1:833, or it could be something that resolves to an IP address, like a domain name.
//!
//! `DnsAddr` attempt to resolve *name-based* multiaddrs into IP addresses before calling other transports. The current multiaddr protocol table defines four resolvable or "name-based" protocols:
//!
//! |protocol	|description|
//! |-----------|-----------|
//! |dns	    |Resolves DNS A and AAAA records into both IPv4 and IPv6 addresses.|
//! |dns4	    |Resolves DNS A records into IPv4 addresses.|
//! |dns6	    |Resolves DNS AAAA records into IPv6 addresses.|
//! |dnsaddr	|Resolves multiaddrs from a special TXT record.|
//!
//! [**IP and Name Resolution**]: https://github.com/libp2p/specs/blob/master/addressing/README.md#ip-and-name-resolution

use std::io::Result;

use async_trait::async_trait;
use futures_dnsv2::client::DnsLookup;
use rand::seq::SliceRandom;
use rand::thread_rng;
use xstack::multiaddr::Multiaddr;
use xstack::transport_syscall::DriverTransport;
use xstack::Switch;
use xstack::{P2pConn, TransportListener};

/// A transport that resolve *name-based* multiaddrs into IP addresses.
///
/// Add [**IP and Name Resolution**] capabilities to **XSTACK** by:
///
/// ```no_run
/// use xstack::Switch;
/// use xstack_dnsaddr::DnsAddr;
///
/// # async fn boostrap() {
/// let switch = Switch::new("test")
///       .transport(DnsAddr::new().await.unwrap())
///       .create()
///       .await
///       .unwrap();
///
/// let (stream,_) = switch
///       .connect(
///            "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
///            ["/ipfs/kad/1.0.0"]
///       ).await.unwrap();
/// # }
/// ```
/// [**IP and Name Resolution**]: https://github.com/libp2p/specs/blob/master/addressing/README.md#ip-and-name-resolution
pub struct DnsAddr(DnsLookup, usize);

impl DnsAddr {
    /// Create an instance of `DnsAddr` using the system-wide DNS configuration.
    pub async fn new() -> Result<Self> {
        Ok(Self(DnsLookup::over_udp().await?, 10))
    }
}

impl DnsAddr {
    async fn lookup(&self, raddr: &Multiaddr) -> Result<Vec<Multiaddr>> {
        let mut depth = 0;

        let mut parsed = vec![];
        let mut dnsaddrs = vec![raddr.clone()];

        while !dnsaddrs.is_empty() {
            if depth > self.1 {
                break;
            }

            let mut cached = vec![];

            for addr in dnsaddrs.drain(..) {
                let mut raddrs = dns_lookup(&self.0, &addr).await?;

                cached.append(&mut raddrs);
            }

            for addr in cached {
                if self.multiaddr_hit(&addr) {
                    dnsaddrs.push(addr);
                } else {
                    parsed.push(addr);
                }
            }

            depth += 1;
        }

        Ok(parsed)
    }
}

#[allow(unused)]
#[async_trait]
impl DriverTransport for DnsAddr {
    /// Create a server-side socket with provided [`laddr`](Multiaddr).
    async fn bind(&self, switch: &Switch, laddr: &Multiaddr) -> Result<TransportListener> {
        panic!("DnsAddr is not support for `DriverTransport::bind` fn.");
    }

    /// Connect to peer with remote peer [`raddr`](Multiaddr).
    async fn connect(&self, switch: &Switch, raddr: &Multiaddr) -> Result<P2pConn> {
        let mut raddrs = self.lookup(raddr).await?;

        if raddrs.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                format!("Can't resolve raddr {}", raddr),
            ));
        }

        raddrs.shuffle(&mut thread_rng());

        let mut last_error = None;

        for raddr in raddrs {
            match switch.transport_connect(&raddr).await {
                Ok(conn) => return Ok(conn),
                Err(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap().into())
    }

    /// Check if this transport support the protocol stack represented by the `addr`.
    fn multiaddr_hit(&self, addr: &Multiaddr) -> bool {
        for protocol in addr.iter() {
            match protocol {
                xstack::multiaddr::Protocol::Dns(_) => return true,
                xstack::multiaddr::Protocol::Dns4(_) => return true,
                xstack::multiaddr::Protocol::Dns6(_) => return true,
                xstack::multiaddr::Protocol::Dnsaddr(_) => return true,
                _ => {}
            }
        }

        false
    }
}

/// Convert dns-* protocol.
async fn dns_lookup(lookup: &DnsLookup, addr: &Multiaddr) -> Result<Vec<Multiaddr>> {
    let mut protocol_stacks: Vec<Multiaddr> = vec![];

    for protocol in addr {
        match protocol {
            xstack::multiaddr::Protocol::Dns(name)
            | xstack::multiaddr::Protocol::Dns4(name)
            | xstack::multiaddr::Protocol::Dns6(name) => {
                let addrs = lookup.lookup_ip(name).await?;

                let mut stacks = vec![];

                for addr in &addrs {
                    if protocol_stacks.is_empty() {
                        stacks.push(addr.clone().into());
                    } else {
                        for mut stack in protocol_stacks.iter().cloned() {
                            stack.push(addr.clone().into());
                            stacks.push(stack);
                        }
                    }
                }

                protocol_stacks = stacks;
            }
            xstack::multiaddr::Protocol::Dnsaddr(name) => {
                let txt = lookup.lookup_txt(format!("_dnsaddr.{}", name)).await?;

                for addr in txt {
                    static PREFIX: &str = "dnsaddr=";

                    if addr.starts_with(PREFIX) {
                        protocol_stacks.push(
                            addr[PREFIX.len()..].parse().map_err(|err| {
                                std::io::Error::new(std::io::ErrorKind::Other, err)
                            })?,
                        );
                    } else {
                        log::warn!("skip unknown dnsaddr text {}", addr);
                    }
                }

                break;
            }
            _ => {
                for stack in protocol_stacks.iter_mut() {
                    stack.push(protocol.clone());
                }
            }
        }
    }

    Ok(protocol_stacks)
}

#[cfg(test)]
mod tests {

    use std::sync::Once;

    use rasi_mio::{net::register_mio_network, timer::register_mio_timer};

    use super::*;

    fn init() {
        static INIT: Once = Once::new();

        INIT.call_once(|| {
            pretty_env_logger::init_timed();
            register_mio_network();
            register_mio_timer();
        });
    }

    #[futures_test::test]
    async fn test_lookup() {
        init();

        let transport = DnsAddr::new().await.unwrap();

        let addrs = transport
            .lookup(
                &"/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN"
                    .parse()
                    .unwrap(),
            )
            .await
            .unwrap();

        log::trace!("{:#?}", addrs);
    }
}
