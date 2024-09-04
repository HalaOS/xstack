use xstack::multiaddr::{self, Multiaddr};

/// The error type of this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    XSTACK(#[from] xstack::Error),

    #[error("k-bucket, call closest with local peer_id")]
    Closest,

    #[error(transparent)]
    PeerIdParseError(#[from] xstack::identity::ParseError),

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

    #[error("{0}")]
    Other(String),

    #[error("Muliaddr is not end with p2p protocol, {0}")]
    WithoutP2p(Multiaddr),

    #[error("Kademlia rpc timeout")]
    Timeout,
}

/// `Result` returns by functions in this crate.
pub type Result<T> = std::result::Result<T, Error>;

impl From<Error> for std::io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::IoError(err) => err,
            err => std::io::Error::new(std::io::ErrorKind::Other, err),
        }
    }
}
