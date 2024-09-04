use std::{collections::HashMap, num::NonZeroUsize, sync::Arc};

use futures::{lock::Mutex, AsyncReadExt, AsyncWriteExt, StreamExt};
use protobuf::{Message, MessageField};
use rasi::task::spawn_ok;
use xstack::{
    events, identity::PeerId, multiaddr::Multiaddr, AutoNAT, Error, EventSource, PeerInfo, Result,
    Switch,
};

use crate::{proto::autonat, PROTOCOL_LIBP2P_AUTONAT};

#[derive(Default)]
struct RawAutoNatClient {
    dial_succ: HashMap<Multiaddr, usize>,
    dial_failed: usize,
}

impl RawAutoNatClient {
    /// reset the autonat state.
    #[allow(unused)]
    fn reset(&mut self) {
        self.dial_succ.clear();
        self.dial_failed = 0;
    }

    fn success(&mut self, addr: Multiaddr) -> (AutoNAT, AutoNAT) {
        let before = self.state();

        if before == AutoNAT::Unknown {
            if let Some(counter) = self.dial_succ.get_mut(&addr) {
                *counter += 1;
            } else {
                self.dial_succ.insert(addr, 1);
            }
        }

        (before, self.state())
    }

    fn failed(&mut self) -> (AutoNAT, AutoNAT) {
        let before = self.state();

        if before == AutoNAT::Unknown {
            self.dial_failed += 1;
        }

        (before, self.state())
    }

    fn state(&self) -> AutoNAT {
        log::trace!(target:"autonat_client","dial_succ={}, dial_failed={}",self.dial_succ.len(),self.dial_failed);
        if self
            .dial_succ
            .iter()
            .map(|(_, counter)| *counter)
            .sum::<usize>()
            > 2
        {
            AutoNAT::Public
        } else if self.dial_failed > 2 {
            AutoNAT::NAT
        } else {
            AutoNAT::Unknown
        }
    }
}

/// The client-side implementation of [**autonat protcol**]
///
/// [**autonat protcol**]: https://github.com/libp2p/specs/tree/master/autonat
#[derive(Clone)]
pub struct AutoNatClient {
    switch: Switch,
    raw: Arc<Mutex<RawAutoNatClient>>,
}

impl AutoNatClient {
    /// Bind a new *autonat* client instance to global context `Switch`.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub fn bind() -> Self {
        use xstack::global_switch;

        Self::bind_with(global_switch())
    }

    /// Bind a new *autonat* client instance to `Switch`
    pub fn bind_with(switch: &Switch) -> Self {
        let client = Self {
            switch: switch.clone(),
            raw: Default::default(),
        };

        spawn_ok(client.clone().auto_nat_client());

        client
    }

    async fn auto_nat_client(self) {
        let mut event_source: EventSource<events::HandshakeSuccess> =
            EventSource::bind_with(&self.switch, NonZeroUsize::new(100).unwrap()).await;

        while let Some((_, peer_id)) = event_source.next().await {
            if AutoNAT::Unknown == self.switch.nat().await {
                if let Ok(Some(peer_info)) = self.switch.lookup_peer_info(&peer_id).await {
                    if peer_info
                        .protos
                        .iter()
                        .find(|proto| proto.as_str() == PROTOCOL_LIBP2P_AUTONAT)
                        .is_some()
                    {
                        spawn_ok(self.clone().autonat_request(peer_id));
                    }
                }
            }
        }
    }

    async fn autonat_request(self, peer_id: PeerId) {
        if let Err(err) = self.autonat_request_prv(&peer_id).await {
            log::error!("autonat_client request, peer_id={}, err={}", peer_id, err);
        }
    }
    async fn autonat_request_prv(&self, peer_id: &PeerId) -> Result<()> {
        log::trace!("autonat_client request, peer_id={}", peer_id);

        let (mut stream, _) = self
            .switch
            .connect(peer_id, [PROTOCOL_LIBP2P_AUTONAT])
            .await?;

        let local_id = self.switch.local_id().clone();

        let laddrs = self.switch.local_addrs().await;

        let peer_info = PeerInfo {
            id: local_id,
            addrs: laddrs,
            ..Default::default()
        };

        let mut message = autonat::Message::new();

        let mut dial = autonat::message::Dial::new();

        dial.peer = MessageField::some(autonat::message::PeerInfo {
            id: Some(peer_info.id.to_bytes()),
            addrs: peer_info.addrs.iter().map(|addr| addr.to_vec()).collect(),
            ..Default::default()
        });

        message.type_ = Some(autonat::message::MessageType::DIAL.into());
        message.dial = MessageField::some(dial);

        let buf = message.write_to_bytes()?;

        let mut payload_len = unsigned_varint::encode::usize_buffer();

        stream
            .write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
            .await?;

        stream.write_all(&buf).await?;

        let body_len = unsigned_varint::aio::read_usize(&mut stream).await?;

        if self.switch.max_packet_size < body_len {
            return Err(Error::Overflow(body_len));
        }

        let mut buf = vec![0; body_len];

        stream.read_exact(&mut buf).await?;

        let message = autonat::Message::parse_from_bytes(&buf)?;

        let response = message
            .dialResponse
            .into_option()
            .ok_or(Error::AutoNatResponse)?;

        log::trace!(
            "autonat_client response from={}, status={:?}",
            stream.public_key().to_peer_id(),
            response.status()
        );

        match response.status() {
            autonat::message::ResponseStatus::OK => {
                if let Some(addr) = response.addr {
                    let addr = Multiaddr::try_from(addr)?;

                    self.auto_nat_success(addr).await;
                }
            }
            _ => {
                self.auto_nat_failed().await;
            }
        }

        Ok(())
    }

    async fn auto_nat_success(&self, addr: Multiaddr) {
        let (before, current) = self.raw.lock().await.success(addr);

        if before != current {
            self.switch.set_nat(current).await;
        }
    }

    async fn auto_nat_failed(&self) {
        let (before, current) = self.raw.lock().await.failed();

        if before != current {
            self.switch.set_nat(current).await;
        }
    }
}
