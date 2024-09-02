use std::{
    future::Future,
    time::{Duration, SystemTime},
};

use futures::{AsyncRead, AsyncWrite};
use protobuf::{EnumOrUnknown, MessageField};

use crate::{
    proto::circuit::{self, hop_message, stop_message, HopMessage, Status, StopMessage},
    Error, Result,
};

use xstack::{identity::PeerId, multiaddr::Multiaddr, XStackRpc};

/// Result of [`circuit_v2_hop_reserve`](CircuitV2Rpc::circuit_v2_hop_reserve) function
#[derive(Debug, Clone)]
pub struct Reservation {
    /// expiration time of the voucher
    pub expire: Option<SystemTime>,
    /// the public relay addrs, including the peer ID of the relay node but not the trailing p2p-circuit part;
    pub addrs: Vec<Multiaddr>,
    /// When omitted, it indicates that the relay does not apply any limits.
    pub limit: Option<Limit>,
}

/// provides information about the limits applied by the relay in relayed connection
#[derive(Debug, Clone)]
pub struct Limit {
    /// the maximum duration of a relayed connection in seconds; if None, there is no limit applied
    pub duration: Option<Duration>,
    /// the maximum number of bytes allowed to be transmitted in each direction; if None there is no limit applied
    pub data: Option<usize>,
}

impl From<circuit::Limit> for Limit {
    fn from(limit: circuit::Limit) -> Self {
        Limit {
            duration: limit.duration.map(|dur| Duration::from_secs(dur as u64)),
            data: limit.data.map(|data| data as usize),
        }
    }
}

impl From<Limit> for circuit::Limit {
    fn from(limit: Limit) -> Self {
        circuit::Limit {
            duration: limit.duration.map(|dur| dur.as_secs() as u32),
            data: limit.data.map(|data| data as u64),
            ..Default::default()
        }
    }
}

/// An extension trait for circuit v2 protocol.
pub trait CircuitV2Rpc: AsyncRead + AsyncWrite + Unpin {
    /// make a reservation to relayer via this stream.
    fn circuit_v2_hop_reserve<M>(
        self,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<Reservation>>
    where
        Self: Sized,
    {
        let mut message = HopMessage::new();

        message.type_ = Some(circuit::hop_message::Type::RESERVE.into());

        async move {
            let message = self.xstack_call(&message, max_recv_len).await?;

            if message.type_ != Some(hop_message::Type::STATUS.into()) {
                return Err(crate::Error::HotReject(
                    "Invalid response, expect status".to_owned(),
                ));
            }

            match message.status {
                Some(status) => {
                    if EnumOrUnknown::from(Status::OK) != status {
                        return Err(crate::Error::HotReject(format!("{:?}", status)));
                    }
                }
                _ => return Err(crate::Error::HotReject("status field is null".to_owned())),
            }

            if let Some(reservation) = message.reservation.into_option() {
                return Ok(Reservation {
                    expire: reservation
                        .expire
                        .map(|value| SystemTime::UNIX_EPOCH + Duration::from_secs(value)),
                    addrs: reservation
                        .addrs
                        .into_iter()
                        .map(|buf| Multiaddr::try_from(buf).map_err(Into::into))
                        .collect::<Result<Vec<_>>>()?,

                    limit: message.limit.into_option().map(|limit| limit.into()),
                });
            }

            return Err(crate::Error::HotReject(
                "reservation field is null".to_owned(),
            ));
        }
    }

    /// make a connect to relayer via this stream.
    fn circuit_v2_hop_connect(
        self,
        id: &PeerId,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<Option<Limit>>>
    where
        Self: Sized,
    {
        let mut message = HopMessage::new();

        message.type_ = Some(circuit::hop_message::Type::CONNECT.into());

        message.peer = MessageField::some(circuit::Peer {
            id: Some(id.to_bytes()),
            // we don't support `Active relay functionality`
            ..Default::default()
        });

        async move {
            let message = self.xstack_call(&message, max_recv_len).await?;

            if message.type_ != Some(hop_message::Type::STATUS.into()) {
                return Err(crate::Error::HotReject(
                    "Invalid response, expect status".to_owned(),
                ));
            }

            match message.status {
                Some(status) => {
                    if EnumOrUnknown::from(Status::OK) != status {
                        return Err(crate::Error::HotReject(format!("{:?}", status)));
                    }
                }
                _ => return Err(crate::Error::HotReject("status field is null".to_owned())),
            }

            Ok(message.limit.into_option().map(|limit| limit.into()))
        }
    }

    /// make a connect to terminator via this stream.
    fn circuit_v2_stop_connect(
        self,
        id: PeerId,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
    {
        let mut message = StopMessage::new();

        message.type_ = Some(circuit::stop_message::Type::CONNECT.into());

        message.peer = MessageField::some(circuit::Peer {
            id: Some(id.to_bytes()),
            // we don't support `Active relay functionality`
            ..Default::default()
        });

        async move {
            let message = self.xstack_call(&message, max_recv_len).await?;

            if message.type_ != Some(stop_message::Type::STATUS.into()) {
                return Err(crate::Error::HotReject(
                    "Invalid response, expect status".to_owned(),
                ));
            }

            match message.status {
                Some(status) => {
                    if EnumOrUnknown::from(Status::OK) != status {
                        return Err(crate::Error::HotReject(format!("{:?}", status)));
                    }
                }
                _ => return Err(crate::Error::HotReject("status field is null".to_owned())),
            }

            Ok(())
        }
    }

    /// make a connect to relayer via this stream.
    fn circuit_v2_stop_connect_accept(
        mut self,
        max_recv_len: usize,
    ) -> impl Future<Output = Result<Option<Limit>>>
    where
        Self: Sized,
    {
        async move {
            let stop_message =
                XStackRpc::xstack_recv::<StopMessage>(&mut self, max_recv_len).await?;

            if stop_message.type_ != Some(stop_message::Type::CONNECT.into()) {
                return Err(Error::CircuitStop("expect CONNECT".to_owned()).into());
            }

            let mut response = StopMessage::new();

            response.type_ = Some(stop_message::Type::STATUS.into());

            response.status = Some(Status::OK.into());
            XStackRpc::xstack_send(&mut self, &response).await?;

            Ok(stop_message.limit.into_option().map(|limits| limits.into()))
        }
    }
}

impl<S> CircuitV2Rpc for S where S: AsyncRead + AsyncWrite + Unpin {}
