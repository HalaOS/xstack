use std::{str::Utf8Error, string::FromUtf8Error};

use libp2p_identity::PeerId;
use multiaddr::Multiaddr;
use multistream_select::NegotiationError;

/// A error variant, returns by switch apis.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("This switch does not support the transport binding/connecting with addr={0}")]
    UnspportMultiAddr(Multiaddr),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error("Routing path does not exist: {0}")]
    RoutingPath(PeerId),

    #[error(transparent)]
    UnsignedVarint(#[from] unsigned_varint::io::ReadError),

    #[error("The receiving packet length is overflow: {0}")]
    Overflow(usize),

    #[error(transparent)]
    ProtobufError(#[from] protobuf::Error),

    #[error(transparent)]
    IdentityDecodingError(#[from] libp2p_identity::DecodingError),

    #[error("Check connection peer_id failed")]
    AuthenticateFailed,

    #[error(transparent)]
    Multiaddr(#[from] multiaddr::Error),

    #[error(transparent)]
    NegotiationError(#[from] NegotiationError),

    #[error("Received invalid ping packet, len={0}")]
    InvalidPingLength(usize),

    #[error("Protocol timeout.")]
    Timeout,

    #[error("{0}")]
    Other(String),

    #[error("Can't bind listener on '{0}'")]
    BindError(String),

    #[error("Protocol listener is not exist or is closed, '{0}'")]
    ProtocolListener(usize),

    #[error("Protocol listener can't bind to {0}")]
    ProtocolListenerBind(String),

    #[error("creating networking stack with null transport stack.")]
    NullTransportStack,

    #[error("Can't find peer in the book.")]
    PeerNotFound,

    #[error("Connect peer address list is empty.")]
    EmptyPeerAddrs,

    #[error("Received mismatched ping response")]
    Ping,

    #[error("Invalid autonat response packet received")]
    AutoNatResponse,

    #[error(transparent)]
    Utf8Error(Utf8Error),

    #[error(transparent)]
    FromUtf8Error(FromUtf8Error),
}

/// The result type for this module.
pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for std::io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::IoError(err) => err,
            err => std::io::Error::new(std::io::ErrorKind::Other, err),
        }
    }
}
