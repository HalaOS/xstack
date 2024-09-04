use std::time::SystemTime;

use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p_identity::{PeerId, PublicKey};
use multiaddr::Multiaddr;

use protobuf::Message;

use crate::{
    proto::identity::Identity, Error, Event, P2pConn, PeerInfo, ProtocolStream, Result, XStackRpc,
};

use super::Switch;

/// protocol name of libp2p identity
pub const PROTOCOL_IPFS_ID: &str = "/ipfs/id/1.0.0";

/// protocol name of libp2p identity push
pub const PROTOCOL_IPFS_PUSH_ID: &str = "/ipfs/id/push/1.0.0";

/// protocol name of libp2p ping
pub const PROTOCOL_IPFS_PING: &str = "/ipfs/ping/1.0.0";

impl Switch {
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
        let identity = XStackRpc::xstack_recv_identity(&mut stream, self.max_packet_size).await?;

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

        Ok(())
    }

    /// Start a "/ipfs/id/1.0.0" handshake.
    pub(super) async fn identity_request(&self, conn: &mut P2pConn) -> Result<()> {
        let conn_peer_id = conn.public_key().to_peer_id();

        let (stream, _) = conn.connect([PROTOCOL_IPFS_ID]).await?;

        match self.identity_push(&conn_peer_id, stream).await {
            Ok(_) => {
                self.ops
                    .stream_dispatcher
                    .handshake_success(conn.id())
                    .await;

                self.ops
                    .event_mediator
                    .raise(Event::HandshakeSuccess {
                        conn_id: conn.id().to_owned(),
                        peer_id: conn_peer_id.to_owned(),
                    })
                    .await;
                return Ok(());
            }
            Err(err) => {
                self.ops.stream_dispatcher.handshake_failed(conn.id()).await;

                self.ops
                    .event_mediator
                    .raise(Event::HandshakeFailed {
                        conn_id: conn.id().to_owned(),
                        peer_id: conn_peer_id.to_owned(),
                    })
                    .await;
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

        identity.set_agentVersion(self.ops.agent_version.to_owned());

        identity.listenAddrs = self
            .listen_addrs()
            .await
            .iter()
            .map(|addr| addr.to_vec())
            .collect::<Vec<_>>();

        identity.protocols = self.ops.stream_dispatcher.protos().await;

        let buf = identity.write_to_bytes()?;

        let mut payload_len = unsigned_varint::encode::usize_buffer();

        stream
            .write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
            .await?;

        stream.write_all(&buf).await?;

        Ok(())
    }
}
