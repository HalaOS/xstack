#![cfg_attr(docsrs, feature(doc_cfg))]

use std::{
    collections::HashMap,
    net::Ipv4Addr,
    str::{from_utf8, FromStr, Utf8Error},
    time::Duration,
};

use dns_parser::{Packet, QueryClass, QueryType, RData, ResponseCode};
use hickory_proto::{
    error::ProtoError,
    op::{Message, MessageType},
    rr::{
        rdata::{PTR, TXT},
        Name, Record, RecordData,
    },
};
use rasi::{net::UdpSocket, task::spawn_ok, timer::sleep};
use uuid::Uuid;
use xstack::{identity::PeerId, multiaddr::Multiaddr, PeerInfo, Switch};

const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MULTICAST_PORT: u16 = 5353;
const QNAME: &str = "_p2p._udp.local";

/// Error variant returns by this mod.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Utf8Error(#[from] Utf8Error),

    #[error(transparent)]
    ProtoError(#[from] ProtoError),
}

/// Result type returns by this module's functions.
pub type Result<T> = std::result::Result<T, Error>;

/// An implementation for libp2p [`mdns`] protocol
///
/// [`mdns`]: https://github.com/libp2p/specs/blob/master/discovery/mdns.md
#[derive(Clone)]
pub struct MdnsProtocol {
    ttl: Duration,
    id: String,
    switch: Switch,
    socket: UdpSocket,
}

impl MdnsProtocol {
    /// Bind a new *autonat* client instance to global context `Switch`.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub async fn bind() -> Result<Self> {
        use xstack::global_switch;

        Self::bind_with(global_switch()).await
    }

    /// Bind a new *autonat* client instance to `Switch`
    pub async fn bind_with(switch: &Switch) -> Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, MULTICAST_PORT)).await?;

        socket.set_multicast_loop_v4(true)?;
        socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;

        let this = Self {
            id: Uuid::new_v4().to_string(),
            ttl: Duration::from_secs(60),
            switch: switch.clone(),
            socket,
        };

        log::trace!("start protocol with id={}", this.id);

        spawn_ok(this.clone().send_loop());
        spawn_ok(this.clone().recv_loop());

        Ok(this)
    }

    /// Close the `mdns` protocol.
    pub fn close(&self) -> std::io::Result<()> {
        self.socket.shutdown(std::net::Shutdown::Both)
    }

    async fn send_loop(self) {
        if let Err(err) = self.send_loop_prv().await {
            log::error!("mdns: send loop stopped with error: {}", err);
        }
    }

    async fn send_loop_prv(self) -> Result<()> {
        loop {
            log::trace!("send mdns message.");

            let id = rand::random();

            let mut builder = dns_parser::Builder::new_query(id, false);

            builder.add_question(
                &format!("{}.{}", self.id, QNAME),
                false,
                QueryType::PTR,
                QueryClass::IN,
            );

            let buf = builder.build().unwrap();

            self.socket
                .send_to(buf, (MULTICAST_ADDR, MULTICAST_PORT))
                .await?;

            sleep(self.ttl).await;
        }
    }

    async fn recv_loop(self) {
        if let Err(err) = self.recv_loop_prv().await {
            log::error!("mdns: recv loop stopped with error: {}", err);
        }
    }

    async fn recv_loop_prv(self) -> Result<()> {
        let mut buf = vec![0; 4096];
        loop {
            let (recv_size, from) = self.socket.recv_from(&mut buf).await?;

            let packet = match dns_parser::Packet::parse(&buf[..recv_size]) {
                Ok(packet) => packet,
                Err(err) => {
                    log::error!("recv mdns from {}, parsed with error: {}", from, err);
                    continue;
                }
            };

            log::trace!("{:?}", packet);

            if packet.header.query {
                self.handle_request(packet).await?;
                continue;
            }

            if packet.header.response_code != ResponseCode::NoError {
                log::trace!("response: {}, {}", from, packet.header.response_code);
                continue;
            }

            if let Err(err) = self.handle_response(packet).await {
                log::error!("handle response with error: {}", err);
            }
        }
    }

    async fn handle_request<'a>(&self, packet: Packet<'a>) -> Result<()> {
        if packet
            .questions
            .iter()
            .find(|question| {
                let qname = question.qname.to_string();

                if qname.ends_with(QNAME) && !qname.starts_with(&self.id) {
                    true
                } else {
                    false
                }
            })
            .is_none()
        {
            log::trace!(
                "skip request. {:?}",
                packet
                    .questions
                    .iter()
                    .map(|question| question.qname.to_string())
                    .collect::<Vec<_>>()
            );
            return Ok(());
        }

        log::trace!("mdns broadcast :{:?}", self.switch.local_addrs().await);

        let name = Name::from_ascii(format!("{}.{}", self.id, QNAME))?;

        let mut message = Message::new();

        message
            .set_id(packet.header.id)
            .set_message_type(MessageType::Response);

        message.add_answer(Record::from_rdata(
            Name::from_ascii(QNAME)?,
            self.ttl.as_secs() as u32,
            PTR(name.clone()).into_rdata(),
        ));

        let addrs = self
            .switch
            .local_addrs()
            .await
            .into_iter()
            .map(|addr| {
                format!(
                    "dnsaddr={}",
                    addr.with_p2p(self.switch.local_id().clone()).unwrap()
                )
            })
            .collect::<Vec<_>>();

        message.add_additional(Record::from_rdata(
            name,
            self.ttl.as_secs() as u32,
            TXT::new(addrs).into_rdata(),
        ));

        log::trace!("mdns response: {}", message);

        self.socket
            .send_to(message.to_vec()?, (MULTICAST_ADDR, MULTICAST_PORT))
            .await?;

        Ok(())
    }

    async fn handle_response<'a>(&self, packet: Packet<'a>) -> Result<()> {
        for answer in &packet.answers {
            if answer.name.to_string().as_str() == QNAME {
                if let RData::PTR(record) = answer.data {
                    let id = record.0.to_string();

                    if id.starts_with(&self.id) {
                        log::trace!("skip self sent response.");
                        return Ok(());
                    }

                    log::trace!("handle response from: {}, ttl={}", id, answer.ttl);

                    for addtion in &packet.additional {
                        if let RData::TXT(record) = &addtion.data {
                            let txt = record
                                .iter()
                                .map(|x| from_utf8(x).map_err(|err| Error::Utf8Error(err)))
                                .collect::<Result<Vec<_>>>()?
                                .concat();

                            let mut peer_addrs = HashMap::<PeerId, Vec<Multiaddr>>::new();

                            for mut raddr in txt
                                .split("dnsaddr=")
                                .flat_map(|addr| Multiaddr::from_str(addr).ok())
                            {
                                match raddr.pop() {
                                    Some(xstack::multiaddr::Protocol::P2p(id)) => {
                                        if let Some(addrs) = peer_addrs.get_mut(&id) {
                                            addrs.push(raddr);
                                        } else {
                                            peer_addrs.insert(id, vec![raddr]);
                                        }
                                    }
                                    _ => {
                                        continue;
                                    }
                                }
                            }

                            for (id, addrs) in peer_addrs {
                                log::trace!("mdns add: id={}, addrs={:?}", id, addrs);
                                _ = self
                                    .switch
                                    .insert_peer_info(PeerInfo {
                                        id,
                                        addrs,
                                        ..Default::default()
                                    })
                                    .await;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Once, time::Duration};

    use rasi::timer::sleep;
    use rasi_mio::{net::register_mio_network, timer::register_mio_timer};
    use xstack::Switch;
    use xstack_tcp::TcpTransport;

    use crate::MdnsProtocol;

    async fn init() -> Switch {
        static INIT: Once = Once::new();

        INIT.call_once(|| {
            _ = pretty_env_logger::try_init_timed();

            register_mio_network();
            register_mio_timer();
        });

        Switch::new("kad-test")
            .transport(TcpTransport::default())
            .transport_bind(["/ip4/0.0.0.0/tcp/0"])
            .create()
            .await
            .unwrap()
    }

    #[futures_test::test]
    async fn test_mdns() {
        let switch = init().await;

        let _mdns = MdnsProtocol::bind_with(&switch).await.unwrap();

        sleep(Duration::from_secs(10000)).await;
    }
}
