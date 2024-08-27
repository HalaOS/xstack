use std::future::Future;

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use identity::PeerId;
use protobuf::{Message, MessageField};

use xstack::{book::PeerInfo, multiaddr::Multiaddr};

use crate::{
    errors::{Error, Result},
    proto::{self, rpc},
};

/// Returns by [`kad_get_providers`](KadRpc::kad_get_providers)
pub struct GetProviders {
    /// Closer peers to the provider key.
    pub closer_peers: Vec<PeerInfo>,
    /// [`PeerInfo`]s of provider.
    pub provider_peers: Vec<PeerInfo>,
}

/// Returns by [`kad_get_value`](KadRpc::kad_get_value)
pub struct GetValue {
    /// Closer peers to the provider key.
    pub closer_peers: Vec<PeerInfo>,
    /// Record value.
    pub value: Option<Vec<u8>>,
}

/// An extension to add kad rpc functions to [`AsyncWrite`] + [`AsyncRead`]
pub trait KadRpc: AsyncWrite + AsyncRead + Unpin {
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
                return Err(Error::ResponeLength(max_recv_len));
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
                    conn_type: peer.connection.enum_value_or_default().into(),
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
                return Err(Error::PutValueReturn("The key mismatch".to_string()));
            }

            if message.type_ != resp.type_ {
                return Err(Error::PutValueReturn("The type mismatch".to_string()));
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

impl<T> KadRpc for T where T: AsyncWrite + AsyncRead + Unpin {}

impl From<proto::rpc::message::ConnectionType> for xstack::book::ConnectionType {
    fn from(value: proto::rpc::message::ConnectionType) -> Self {
        match value {
            rpc::message::ConnectionType::NOT_CONNECTED => Self::NotConnected,
            rpc::message::ConnectionType::CONNECTED => Self::Connected,
            rpc::message::ConnectionType::CAN_CONNECT => Self::CanConnect,
            rpc::message::ConnectionType::CANNOT_CONNECT => Self::CannotConnect,
        }
    }
}

impl From<xstack::book::ConnectionType> for proto::rpc::message::ConnectionType {
    fn from(value: xstack::book::ConnectionType) -> Self {
        match value {
            xstack::book::ConnectionType::NotConnected => Self::NOT_CONNECTED,
            xstack::book::ConnectionType::Connected => Self::CONNECTED,
            xstack::book::ConnectionType::CanConnect => Self::CAN_CONNECT,
            xstack::book::ConnectionType::CannotConnect => Self::CANNOT_CONNECT,
        }
    }
}

impl From<PeerInfo> for proto::rpc::message::Peer {
    fn from(value: PeerInfo) -> Self {
        Self::from(&value)
    }
}
impl From<&PeerInfo> for proto::rpc::message::Peer {
    fn from(value: &PeerInfo) -> Self {
        Self {
            id: value.id.to_bytes(),
            addrs: value.addrs.iter().map(|addr| addr.to_vec()).collect(),
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
