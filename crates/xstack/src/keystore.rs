//! A utility to load node's keypair, used by [`Switch`](super::Switch).

use std::io::Result;

use async_trait::async_trait;
use identity::{Keypair, PublicKey};

use crate::driver_wrapper;

/// A libp2p keystore driver must implement the `Driver-*` traits in this module.
pub mod syscall {
    use std::io::Result;

    use async_trait::async_trait;
    use identity::PublicKey;

    #[async_trait]
    pub trait DriverKeyStore: Sync + Send {
        /// Returns the public key of the host keypair.
        async fn public_key(&self) -> Result<PublicKey>;

        /// Signed the unhashed data use the host private key.
        async fn sign(&self, data: &[u8]) -> Result<Vec<u8>>;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverKeyStore`](syscall::DriverKeyStore)"]
    KeyStore[syscall::DriverKeyStore]
);

/// An in memory [`KeyStore`] implementation
pub struct MemoryKeyStore(Keypair);

impl From<Keypair> for MemoryKeyStore {
    fn from(value: Keypair) -> Self {
        Self(value)
    }
}

impl MemoryKeyStore {
    /// Create a random keystore backed keypair is generated with ed25519 algorithm.
    pub fn random() -> Self {
        Self(Keypair::generate_ed25519())
    }
}

#[async_trait]
impl syscall::DriverKeyStore for MemoryKeyStore {
    /// Returns the public key of the host keypair.
    async fn public_key(&self) -> Result<PublicKey> {
        Ok(self.0.public())
    }

    /// Signed the unhashed data use the host private key.
    async fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.0
            .sign(&data)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
    }
}
