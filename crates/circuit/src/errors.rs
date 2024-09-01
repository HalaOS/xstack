use xstack::multiaddr;

/// The error type of this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    #[error("hot reject: {0}")]
    HotReject(String),

    #[error(transparent)]
    ProtoBuf(#[from] protobuf::Error),

    #[error(transparent)]
    ReadError(#[from] unsigned_varint::io::ReadError),

    #[error("receiving packet length is out of range: {0}")]
    OutOfRange(usize),

    #[error(transparent)]
    XStackError(#[from] xstack::Error),

    #[error(transparent)]
    Multiaddr(#[from] multiaddr::Error),
}

/// The result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;
