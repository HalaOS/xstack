use identity::PeerId;
use multiaddr::Multiaddr;
use multistream_select::NegotiationError;

/// A error variant, returns by switch apis.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("This switch does not support the transport binding/connecting with addr={0}")]
    UnspportMultiAddr(Multiaddr),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error("Connect to peer, id={0}, error='route path is not exists'")]
    ConnectPeer(PeerId),

    #[error(transparent)]
    UnsignedVarint(#[from] unsigned_varint::io::ReadError),

    #[error("Received identity length > {0}")]
    IdentityOverflow(usize),

    #[error(transparent)]
    ProtobufError(#[from] protobuf::Error),

    #[error(transparent)]
    IdentityDecodingError(#[from] identity::DecodingError),

    #[error("Identity check conn({0}) != identify({1})")]
    IdentityCheckFailed(PeerId, PeerId),

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
