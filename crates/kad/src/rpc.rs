use std::future::Future;

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p_identity::PeerId;
use protobuf::{Message, MessageField};
use xstack::{multiaddr::Multiaddr, PeerInfo};

use crate::{
    proto::{
        self,
        rpc::{self, message::ConnectionType},
    },
    Error, Result,
};

/// Returns by [`kad_get_providers`](KademliaRpc::kad_get_providers)
pub struct GetProviders {
    /// Closer peers to the provider key.
    pub closer_peers: Vec<PeerInfo>,
    /// [`PeerInfo`]s of provider.
    pub provider_peers: Vec<PeerInfo>,
}

/// Returns by [`kad_get_value`](KademliaRpc::kad_get_value)
pub struct GetValue {
    /// Closer peers to the provider key.
    pub closer_peers: Vec<PeerInfo>,
    /// Record value.
    pub value: Option<Vec<u8>>,
}

/// An extension trait that add `Kademlia` RPC calls to [`AsyncWrite`] + [`AsyncRead`]
pub trait KademliaRpc: AsyncWrite + AsyncRead + Unpin {
    /// Send a kad request and wait for response.
    fn kad_rpc_call(
        mut self,
        message: &rpc::Message,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<rpc::Message>>
    where
        Self: Sized,
    {
        async move {
            let buf = message.write_to_bytes()?;

            let mut payload_len = unsigned_varint::encode::usize_buffer();

            self.write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
                .await?;

            self.write_all(buf.as_slice()).await?;

            log::trace!("KAD_RPC_CALL, tx length={}", buf.len());

            let body_len = unsigned_varint::aio::read_usize(&mut self).await?;

            log::trace!("KAD_RPC_CALL, rx length={}", body_len);

            if body_len > max_recv_len {
                return Err(Error::OutOfRange(max_recv_len));
            }

            let mut buf = vec![0u8; body_len];

            self.read_exact(&mut buf).await?;

            let message = rpc::Message::parse_from_bytes(&buf)?;

            Ok(message)
        }
    }

    /// Send a kad message.
    fn kad_rpc_send(mut self, message: &rpc::Message) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
    {
        async move {
            let buf = message.write_to_bytes()?;

            let mut payload_len = unsigned_varint::encode::usize_buffer();

            self.write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
                .await?;

            self.write_all(buf.as_slice()).await?;

            log::trace!("KAD_RPC_SEND, tx length={}", buf.len());

            Ok(())
        }
    }

    /// Send a kad `FIND_NODE` request and wait for response.
    fn kad_find_node<K>(
        self,
        key: K,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<Vec<PeerInfo>>>
    where
        Self: Sized,
        K: AsRef<[u8]>,
    {
        let mut message = rpc::Message::new();

        message.type_ = rpc::message::MessageType::FIND_NODE.into();
        message.key = key.as_ref().to_vec();

        async move {
            let message = self.kad_rpc_call(&message, max_recv_len).await?;

            let mut peers = vec![];

            for peer in message.closerPeers {
                let mut addrs = vec![];

                for addr in peer.addrs {
                    addrs.push(Multiaddr::try_from(addr)?);
                }

                peers.push(PeerInfo {
                    id: PeerId::from_bytes(&peer.id)?,
                    addrs,
                    ..Default::default()
                });
            }

            Ok(peers)
        }
    }

    /// Send a kad `PUT_VALUE` request and wait for response.
    fn kad_put_value<K, V>(
        self,
        key: K,
        value: V,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
        K: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let mut record = rpc::Record::new();

        record.key = key.as_ref().to_vec();
        record.value = value.as_ref().to_vec();

        let mut message = rpc::Message::new();

        message.type_ = rpc::message::MessageType::PUT_VALUE.into();
        message.key = record.key.clone();
        message.record = MessageField::some(record);

        async move {
            let resp = self.kad_rpc_call(&message, max_recv_len).await?;

            if message.key != resp.key {
                return Err(Error::RpcType);
            }

            if message.type_ != resp.type_ {
                return Err(Error::RpcType);
            }

            Ok(())
        }
    }

    /// Send a kad `Get_VALUE` request and wait for response.
    fn kad_get_value<K>(self, key: K, max_recv_len: usize) -> impl Future<Output = Result<GetValue>>
    where
        Self: Sized,
        K: AsRef<[u8]>,
    {
        let mut message = rpc::Message::new();

        message.type_ = rpc::message::MessageType::GET_VALUE.into();
        message.key = key.as_ref().to_vec();

        async move {
            let resp = self.kad_rpc_call(&message, max_recv_len).await?;

            let closer_peers = resp
                .closerPeers
                .into_iter()
                .map(|peer| peer.try_into())
                .collect::<Result<Vec<PeerInfo>>>()?;

            let value = if let Some(record) = resp.record.into_option() {
                Some(record.value)
            } else {
                None
            };

            Ok(GetValue {
                closer_peers,
                value,
            })
        }
    }

    /// Send a kad `ADD_PROVIDER` message.
    fn kad_add_provider<K>(self, key: K, peer_info: &PeerInfo) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
        K: AsRef<[u8]>,
    {
        let mut message = rpc::Message::new();

        message.type_ = rpc::message::MessageType::ADD_PROVIDER.into();
        message.key = key.as_ref().to_vec();
        message.providerPeers = vec![peer_info.into()];

        async move {
            self.kad_rpc_send(&message).await?;

            Ok(())
        }
    }

    /// Send a kad `GEt_PROVIDERS` request and wait for response.
    fn kad_get_providers<K>(
        self,
        key: K,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<GetProviders>>
    where
        Self: Sized,
        K: AsRef<[u8]>,
    {
        let mut message = rpc::Message::new();

        message.type_ = rpc::message::MessageType::GET_PROVIDERS.into();
        message.key = key.as_ref().to_vec();

        async move {
            let resp = self.kad_rpc_call(&message, max_recv_len).await?;

            let closer_peers = resp
                .closerPeers
                .into_iter()
                .map(|peer| peer.try_into())
                .collect::<Result<Vec<PeerInfo>>>()?;

            let provider_peers = resp
                .providerPeers
                .into_iter()
                .map(|peer| peer.try_into())
                .collect::<Result<Vec<PeerInfo>>>()?;

            Ok(GetProviders {
                provider_peers,
                closer_peers,
            })
        }
    }
}

impl<T> KademliaRpc for T where T: AsyncWrite + AsyncRead + Unpin {}

impl From<PeerInfo> for proto::rpc::message::Peer {
    fn from(value: PeerInfo) -> Self {
        Self::from(&value)
    }
}
impl From<&PeerInfo> for proto::rpc::message::Peer {
    fn from(value: &PeerInfo) -> Self {
        let connection = if value.appear.is_some() {
            if value.disappear.is_none() {
                ConnectionType::CONNECTED
            } else {
                ConnectionType::CAN_CONNECT
            }
        } else {
            if value.disappear.is_none() {
                ConnectionType::NOT_CONNECTED
            } else {
                ConnectionType::CANNOT_CONNECT
            }
        };

        Self {
            id: value.id.to_bytes(),
            addrs: value.addrs.iter().map(|addr| addr.to_vec()).collect(),
            connection: connection.into(),
            ..Default::default()
        }
    }
}

impl TryFrom<proto::rpc::message::Peer> for PeerInfo {
    type Error = Error;
    fn try_from(value: proto::rpc::message::Peer) -> Result<Self> {
        Ok(Self {
            id: PeerId::from_bytes(&value.id)?,
            addrs: value
                .addrs
                .into_iter()
                .map(|addr| Multiaddr::try_from(addr).map_err(|err| err.into()))
                .collect::<Result<Vec<Multiaddr>>>()?,
            ..Default::default()
        })
    }
}
