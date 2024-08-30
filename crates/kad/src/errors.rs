/// The error type of this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    XSTACK(#[from] xstack::Error),
}

/// `Result` returns by functions in this crate.
pub type Result<T> = std::result::Result<T, Error>;
