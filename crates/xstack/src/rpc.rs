use std::future::Future;

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use protobuf::Message;
use rand::{thread_rng, RngCore};

use crate::{proto::identity::Identity, Error, Result};

/// An extension trait for libp2p rpc calls.
pub trait XStackRpc: AsyncRead + AsyncWrite + Unpin {
    /// make a rpc call.
    ///
    /// # Parameters
    /// - `message`, a protobuf [`Message`].
    ///
    /// On success, returns one same type [`Message`] as inputs one.
    fn xstack_call<M>(mut self, message: &M, max_recv_len: usize) -> impl Future<Output = Result<M>>
    where
        Self: Sized,
        M: Message,
    {
        async move {
            XStackRpc::xstack_send(&mut self, message).await?;

            let body_len = unsigned_varint::aio::read_usize(&mut self).await?;

            if body_len > max_recv_len {
                return Err(Error::Overflow(max_recv_len));
            }

            let mut buf = vec![0u8; body_len];

            self.read_exact(&mut buf).await?;

            Ok(M::parse_from_bytes(&buf)?)
        }
    }

    /// receive one protobuf message.
    ///
    /// # Parameters
    /// - `max_recv_len`, the maximum length of the receiving packet.
    ///
    /// On success, returns one same type [`Message`] as inputs one.
    fn xstack_recv<M>(mut self, max_recv_len: usize) -> impl Future<Output = Result<M>>
    where
        Self: Sized,
        M: Message,
    {
        async move {
            let body_len = unsigned_varint::aio::read_usize(&mut self).await?;

            if body_len > max_recv_len {
                return Err(Error::Overflow(max_recv_len));
            }

            let mut buf = vec![0u8; body_len];

            self.read_exact(&mut buf).await?;

            Ok(M::parse_from_bytes(&buf)?)
        }
    }

    /// make a rpc call. unlike [`xstack_call`](XStackRpc), this function doesn't wait a response.
    ///
    /// # Parameters
    /// - `message`, a protobuf [`Message`].
    fn xstack_send<M>(mut self, message: &M) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
        M: Message,
    {
        // let mut message = HopMessage::new();

        // message.type_ = Some(circuit::hop_message::Type::RESERVE.into());

        async move {
            let buf = message.write_to_bytes()?;

            let mut payload_len = unsigned_varint::encode::usize_buffer();

            self.write_all(unsigned_varint::encode::usize(buf.len(), &mut payload_len))
                .await?;

            self.write_all(buf.as_slice()).await?;

            Ok(())
        }
    }

    /// Make a ping test via the stream.
    fn xstack_ping(mut self) -> impl Future<Output = Result<()>>
    where
        Self: Sized,
    {
        async move {
            let mut buf = vec![0u8; 32];

            thread_rng().fill_bytes(&mut buf);

            self.write_all(&buf).await?;

            let mut echo = vec![0u8; 32];

            self.read_exact(&mut echo).await?;

            if echo != buf {
                return Err(Error::Ping);
            }

            Ok(())
        }
    }

    fn xstack_recv_identity(
        mut self,
        max_packet_size: usize,
    ) -> impl Future<Output = Result<Identity>>
    where
        Self: Sized,
    {
        async move {
            log::trace!("identity_request: read varint length");

            let body_len = unsigned_varint::aio::read_usize(&mut self).await?;

            log::trace!("identity_request: read varint length");

            if max_packet_size < body_len {
                return Err(Error::Overflow(body_len));
            }

            log::trace!("identity_request recv body: {}", body_len);

            let mut buf = vec![0; body_len];

            self.read_exact(&mut buf).await?;

            Ok(Identity::parse_from_bytes(&buf)?)
        }
    }
}

impl<S> XStackRpc for S where S: AsyncRead + AsyncWrite + Unpin {}
