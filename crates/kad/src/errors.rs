use libp2p_identity::ParseError;
use xstack::multiaddr;

/// The error type of this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    XSTACK(#[from] xstack::Error),

    #[error("k-bucket, call closest with local peer_id")]
    Closest,

    #[error(transparent)]
    PeerIdParseError(#[from] ParseError),

    #[error(transparent)]
    MultiaddrError(#[from] multiaddr::Error),

    #[error("The response type is mismatched")]
    RpcType,

    #[error(transparent)]
    ProtoBufError(#[from] protobuf::Error),

    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    UnsignedVarintError(#[from] unsigned_varint::io::ReadError),

    #[error("request/response packet length is out of range.")]
    OutOfRange(usize),
}

/// `Result` returns by functions in this crate.
pub type Result<T> = std::result::Result<T, Error>;
