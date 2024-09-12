use std::{
    fmt::Display,
    future::Future,
    time::{Duration, SystemTime},
    usize,
};

use futures::{AsyncRead, AsyncWrite};
use protobuf::{EnumOrUnknown, MessageField};

use crate::{
    proto::{
        circuit::{self, hop_message, stop_message, HopMessage, Status, StopMessage},
        DCUtR,
    },
    Error, Result,
};

use xstack::{identity::PeerId, multiaddr::Multiaddr, XStackRpc};

/// Result of [`circuit_v2_hop_reserve`](CircuitV2Rpc::circuit_v2_hop_reserve) function
#[derive(Debug, Clone)]
pub struct Reservation {
    /// expiration time of the voucher
    pub expire: SystemTime,
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

impl Display for Limit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "duration={:?}, data={:?}", self.duration, self.data)
    }
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
    fn circuit_v2_hop_reserve(
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
                        .map(|value| SystemTime::UNIX_EPOCH + Duration::from_secs(value))
                        .ok_or(Error::ReservationExpire)?,
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

/// An extension trait for [`DCUtR`] protocol.
///
/// [`DCUtR`]: https://github.com/libp2p/specs/blob/master/relay/DCUtR.md
pub trait DCUtRRpc: AsyncRead + AsyncWrite + Unpin {
    /// Send a `DCUtR Connect` message via this stream.
    fn dcutr_send_connect(self, observed_addrs: &[Multiaddr]) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
    {
        let mut message = DCUtR::HolePunch::new();

        message.type_ = Some(DCUtR::hole_punch::Type::CONNECT.into());
        message.ObsAddrs = observed_addrs.iter().map(|addr| addr.to_vec()).collect();

        async move { Ok(XStackRpc::xstack_send(self, &message).await?) }
    }

    /// Recv a `DCUtR Connect` message via this stream.
    fn dcutr_recv_connect(self, max_recv_len: usize) -> impl Future<Output = Result<Vec<Multiaddr>>>
    where
        Self: Sized,
    {
        async move {
            let message = XStackRpc::xstack_recv::<DCUtR::HolePunch>(self, max_recv_len).await?;

            if message.type_ != Some(DCUtR::hole_punch::Type::CONNECT.into()) {
                return Err(Error::DCUtRConnect);
            }

            let addrs = message
                .ObsAddrs
                .into_iter()
                .map(|addr| Multiaddr::try_from(addr).map_err(|err| err.into()))
                .collect::<Result<Vec<_>>>()?;

            Ok(addrs)
        }
    }

    /// Send a `DCUtR Sync` message via this stream.
    fn dcutr_send_sync(self) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
    {
        let mut message = DCUtR::HolePunch::new();

        message.type_ = Some(DCUtR::hole_punch::Type::SYNC.into());

        async move { Ok(XStackRpc::xstack_send(self, &message).await?) }
    }

    /// Recv a `DCUtR Sync` message via this stream.
    fn dcutr_recv_sync(self, max_recv_len: usize) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
    {
        async move {
            let message = XStackRpc::xstack_recv::<DCUtR::HolePunch>(self, max_recv_len).await?;

            if message.type_ != Some(DCUtR::hole_punch::Type::SYNC.into()) {
                return Err(Error::DCUtRSync);
            }

            Ok(())
        }
    }
}

impl<S> DCUtRRpc for S where S: AsyncRead + AsyncWrite + Unpin {}
