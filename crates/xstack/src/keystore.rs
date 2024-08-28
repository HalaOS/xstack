//! A utility to load node's keypair, used by [`Switch`](super::Switch).

use std::io::Result;

use async_trait::async_trait;
use libp2p_identity::{Keypair, PublicKey};

use crate::driver_wrapper;

/// A libp2p keystore driver must implement the `Driver-*` traits in this module.
///
/// A `KeyStore` is the vault of the security key of the `Switch`.
/// this is part of **XSTACK** modularity, developers can call [`keystore`](crate::SwitchBuilder::keystore)
/// to set the customise implementation, the default implementation used by `Switch` is [`MemoryKeyStore`].
///
/// ## For example
///
/// You can customize a `KeyStore` that may proxy the request to one [`bastion host`](https://www.wikiwand.com/en/articles/Bastion_host).
///
/// ```no_run
/// use xstack::Switch;
///
/// # async fn boostrap() {
/// Switch::new("test")
///       // .keystore(BastionHostKeyStore)
///       .transport_bind(["/ip4/127.0.0.1/tcp/0"])
///       .create()
///       .await
///       .unwrap()
///       // register to global context.
///       .into_global();
/// # }
///```
pub mod keystore_syscall {
    use std::io::Result;

    use async_trait::async_trait;
    use libp2p_identity::PublicKey;

    /// The customize `KeyStore` must implement this trait.
    ///
    /// The stabilization of async functions in traits in Rust 1.75 did not include support for
    /// using traits containing async functions as dyn Trait, so we use the [**async_trait**](https://docs.rs/async-trait/)
    /// crate to define this trait, to know how to implement the async trait, visit its documentation.
    #[async_trait]
    pub trait DriverKeyStore: Sync + Send {
        /// Returns the public key of the node.
        ///
        ///
        /// `Swith` use the public key to:
        ///
        /// * generate [**libp2p x509 Public Key Extension**](https://github.com/libp2p/specs/blob/master/tls/tls.md#libp2p-public-key-extension);
        /// * generate the node's [**PeerId**](crate::identity::PeerId).
        async fn public_key(&self) -> Result<PublicKey>;

        /// Sign input `data` with the security key.
        async fn sign(&self, data: &[u8]) -> Result<Vec<u8>>;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverKeyStore`](keystore_syscall::DriverKeyStore)"]
    KeyStore[keystore_syscall::DriverKeyStore]
);

/// In-memory [`KeyStore`] implementation with random key generation on startup.
///
/// Also, you can create a `MemoryKeyStore` from [`Keypair`]:
/// ```no_run
/// use xstack::{Switch, MemoryKeyStore, identity::Keypair};
///
/// # async fn boostrap() {
/// Switch::new("test")
///       .keystore(MemoryKeyStore::from(Keypair::generate_ed25519()))
///       .transport_bind(["/ip4/127.0.0.1/tcp/0"])
///       .create()
///       .await
///       .unwrap()
///       // register to global context.
///       .into_global();
/// # }
///```
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
impl keystore_syscall::DriverKeyStore for MemoryKeyStore {
    async fn public_key(&self) -> Result<PublicKey> {
        Ok(self.0.public())
    }

    async fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.0
            .sign(&data)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
    }
}
