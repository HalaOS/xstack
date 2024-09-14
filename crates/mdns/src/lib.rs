#![cfg_attr(docsrs, feature(doc_cfg))]

use std::{
    collections::HashMap,
    str::{from_utf8, FromStr},
    time::Duration,
};

use futures::TryStreamExt;
use futures_dnsv2::{
    mdns::{MdnsDiscover, MdnsDiscoverNetwork},
    message::{
        rr::{
            rdata::{PTR, TXT},
            Name, RData, Record, RecordData,
        },
        Message, MessageType,
    },
    Error, Result,
};

use rasi::task::spawn_ok;
use uuid::Uuid;
use xstack::{identity::PeerId, multiaddr::Multiaddr, PeerInfo, Switch};

const QNAME: &str = "_p2p._udp.local";

/// An implementation for libp2p [`mdns`] protocol
///
/// [`mdns`]: https://github.com/libp2p/specs/blob/master/discovery/mdns.md
pub struct MdnsProtocol(MdnsDiscoverNetwork);

impl Drop for MdnsProtocol {
    fn drop(&mut self) {
        self.0.close();
    }
}

impl MdnsProtocol {
    /// Bind a new *autonat* client instance to global context `Switch`.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub async fn bind(intervals: Duration) -> Result<Self> {
        use xstack::global_switch;

        Self::bind_with(global_switch(), intervals).await
    }

    /// Bind a new *autonat* client instance to `Switch`
    pub async fn bind_with(switch: &Switch, intervals: Duration) -> Result<Self> {
        let discover = MdnsDiscover::all(QNAME, intervals).await?;

        let network = discover.to_network();

        spawn_ok(Self::discover_loop(switch.clone(), discover, intervals));

        Ok(Self(network))
    }

    async fn discover_loop(switch: Switch, discover: MdnsDiscover, intervals: Duration) {
        if let Err(err) = Self::discover_loop_prv(switch, discover, intervals).await {
            log::error!("{}", err);
        }
    }

    async fn discover_loop_prv(
        switch: Switch,
        discover: MdnsDiscover,
        intervals: Duration,
    ) -> Result<()> {
        let network = discover.to_network();
        let mut incoming = discover.into_incoming();

        while let Some((message, from)) = incoming.try_next().await? {
            if message.message_type() == MessageType::Query {
                match Self::handle_request(&switch, message, intervals).await {
                    Ok(message) => {
                        network.multicast(message).await?;
                    }
                    Err(err) => {
                        log::error!("handle request from {} with error: {}", from, err);
                    }
                }
            } else {
                if let Err(err) = Self::handle_response(&switch, message).await {
                    log::error!("handle response from {} with error: {}", from, err);
                }
            }
        }

        Ok(())
    }

    async fn handle_request(
        switch: &Switch,
        request: Message,
        intervals: Duration,
    ) -> Result<Message> {
        let mut message = Message::new();

        message
            .set_id(request.id())
            .set_message_type(MessageType::Response);

        let name = Name::from_utf8(format!("{}.{}", Uuid::new_v4(), QNAME))?;

        message.add_answer(Record::from_rdata(
            Name::from_ascii(QNAME)?,
            intervals.as_secs() as u32,
            PTR(name.clone()).into_rdata(),
        ));

        let addrs = switch
            .local_addrs()
            .await
            .into_iter()
            .map(|addr| {
                format!(
                    "dnsaddr={}",
                    addr.with_p2p(switch.local_id().clone()).unwrap()
                )
            })
            .collect::<Vec<_>>();

        message.add_additional(Record::from_rdata(
            name,
            intervals.as_secs() as u32,
            TXT::new(addrs).into_rdata(),
        ));

        Ok(message)
    }

    async fn handle_response<'a>(switch: &Switch, message: Message) -> Result<()> {
        for additional in message.additionals() {
            if let Some(RData::TXT(record)) = additional.data() {
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
                    _ = switch
                        .insert_peer_info(PeerInfo {
                            id,
                            addrs,
                            ..Default::default()
                        })
                        .await;
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

        let _mdns = MdnsProtocol::bind_with(&switch, Duration::from_secs(4))
            .await
            .unwrap();

        sleep(Duration::from_secs(10000)).await;
    }
}
