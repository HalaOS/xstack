use std::time::SystemTime;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p_identity::{PeerId, PublicKey};
use multiaddr::Multiaddr;
use multistream_select::{dialer_select_proto, Version};
use protobuf::{Message, MessageField};
use rasi::task::spawn_ok;

use crate::{
    events,
    proto::{autonat, identity::Identity},
    AutoNAT, Error, EventArgument, EventSource, PeerInfo, ProtocolStream, Result,
    TransportConnection,
};

use super::Switch;

/// protocol name of libp2p identity
pub const PROTOCOL_IPFS_ID: &str = "/ipfs/id/1.0.0";

/// protocol name of libp2p identity push
pub const PROTOCOL_IPFS_PUSH_ID: &str = "/ipfs/id/push/1.0.0";

/// protocol name of libp2p ping
pub const PROTOCOL_IPFS_PING: &str = "/ipfs/ping/1.0.0";

/// String of AutoNAT Protocol
pub const PROTOCOL_LIBP2P_AUTONAT: &str = "/libp2p/autonat/1.0.0";

impl Switch {
    pub(super) async fn auto_nat_client(self) {
        let mut event_source: EventSource<events::Connected> =
            EventSource::bind_with(&self, 100).await;

        while let Some(peer_id) = event_source.next().await {
            log::trace!(target:"autonat_client", "peer_id={}", peer_id);

            if AutoNAT::Unknown == self.auto_nat().await {
                if let Ok(Some(peer_info)) = self.lookup_peer_info(&peer_id).await {
                    log::trace!(target:"autonat_client", "peer_id={}, protos={:?}", peer_id, peer_info.protos);
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
            log::error!(target:"autonat_client", "peer_id={}, err={}", peer_id, err);
        }
    }
    async fn autonat_request_prv(&self, peer_id: &PeerId) -> Result<()> {
        log::trace!(target:"autonat_client", "call peer_id={}", peer_id);

        let (mut stream, _) = self.connect(peer_id, [PROTOCOL_LIBP2P_AUTONAT]).await?;

        let local_id = self.local_id().clone();

        let laddrs = self.local_addrs().await;

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

        if self.immutable.max_packet_size < body_len {
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
            target:"autonat_client",
            "response from={}, status={:?}",
            stream.public_key().to_peer_id(),
            response.status()
        );

        match response.status() {
            autonat::message::ResponseStatus::OK => {
                if let Some(addr) = response.addr {
                    let addr = Multiaddr::try_from(addr)?;

                    self.mutable.lock().await.auto_nat_success(addr);
                }
            }
            _ => {
                self.mutable.lock().await.auto_nat_failed();
            }
        }

        Ok(())
    }

    /// Handle `/ipfs/ping/1.0.0` request.
    pub(super) async fn ping_echo(&self, mut stream: ProtocolStream) -> Result<()> {
        loop {
            log::trace!("recv /ipfs/ping/1.0.0");

            let body_len = unsigned_varint::aio::read_usize(&mut stream).await?;

            log::trace!("recv /ipfs/ping/1.0.0 payload len {}", body_len);

            if body_len != 32 {
                return Err(Error::InvalidPingLength(body_len));
            }

            let mut buf = vec![0; 31];

            stream.read_exact(&mut buf).await?;

            let mut payload_len = unsigned_varint::encode::usize_buffer();

            stream
                .write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
                .await?;

            stream.write_all(&buf).await?;

            log::trace!("send /ipfs/ping/1.0.0 echo");
        }
    }

    pub(super) async fn identity_push(
        &self,
        conn_peer_id: &PeerId,
        mut stream: ProtocolStream,
    ) -> Result<()> {
        let identity = {
            log::trace!("identity_request: read varint length");

            let body_len = unsigned_varint::aio::read_usize(&mut stream).await?;

            log::trace!("identity_request: read varint length");

            if self.immutable.max_packet_size < body_len {
                return Err(Error::Overflow(self.immutable.max_packet_size));
            }

            log::trace!("identity_request recv body: {}", body_len);

            let mut buf = vec![0; body_len];

            stream.read_exact(&mut buf).await?;

            Identity::parse_from_bytes(&buf)?
        };

        let pubkey = PublicKey::try_decode_protobuf(identity.publicKey())?;

        let peer_id = pubkey.to_peer_id();

        if *conn_peer_id != peer_id {
            log::trace!(
                "authenticate peer failed, connection peer_id={}, identity represents peer_id={}",
                conn_peer_id,
                peer_id
            );

            return Err(Error::AuthenticateFailed);
        }

        let raddrs = identity
            .listenAddrs
            .into_iter()
            .map(|buf| Multiaddr::try_from(buf).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;

        let observed_addrs = identity
            .observedAddr
            .into_iter()
            .map(|buf| Multiaddr::try_from(buf).map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;

        //TODO: add nat codes
        log::info!("{} observed addrs: {:?}", peer_id, observed_addrs);

        log::info!("{} protos: {:?}", peer_id, identity.protocols);

        let peer_info = PeerInfo {
            id: peer_id,
            addrs: raddrs,
            protos: identity.protocols,
            appear: Some(SystemTime::now()),
            ..Default::default()
        };

        self.insert_peer_info(peer_info).await?;

        // notify event '/xstack/event/connected'
        let mut mutable = self.mutable.lock().await;

        mutable.notify(EventArgument::Connected(peer_id));

        Ok(())
    }

    /// Start a "/ipfs/id/1.0.0" handshake.
    pub(super) async fn identity_request(&self, conn: &mut TransportConnection) -> Result<()> {
        let mut stream = conn.connect().await?;

        let conn_peer_id = conn.public_key().to_peer_id();

        dialer_select_proto(&mut stream, ["/ipfs/id/1.0.0"], Version::V1).await?;

        match self.identity_push(&conn_peer_id, stream).await {
            Ok(_) => {
                self.mutable.lock().await.conn_handshake_succ(conn.clone());
                return Ok(());
            }
            Err(err) => {
                self.mutable.lock().await.conn_handshake_failed(conn);
                return Err(err);
            }
        }
    }

    pub(super) async fn identity_response(&self, mut stream: ProtocolStream) -> Result<()> {
        log::trace!("handle identity request");

        let peer_addr = stream.peer_addr();

        let mut identity = Identity::new();

        identity.set_observedAddr(peer_addr.to_vec());

        identity.set_publicKey(self.local_public_key().encode_protobuf());

        identity.set_agentVersion(self.immutable.agent_version.to_owned());

        identity.listenAddrs = self
            .local_addrs()
            .await
            .iter()
            .map(|addr| addr.to_vec())
            .collect::<Vec<_>>();

        identity.protocols = self.mutable.lock().await.protos();

        let buf = identity.write_to_bytes()?;

        let mut payload_len = unsigned_varint::encode::usize_buffer();

        stream
            .write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
            .await?;

        stream.write_all(&buf).await?;

        Ok(())
    }
}
