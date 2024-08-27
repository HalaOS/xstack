#![cfg_attr(docsrs, feature(doc_cfg))]

mod book;
pub use book::*;

mod keystore;
pub use keystore::*;

mod transport;
pub use transport::*;

mod switch;
pub use switch::*;

mod errors;
pub use errors::*;

pub mod multiaddr;
/// A node's network identity keys.
pub mod identity {
    pub use libp2p_identity::*;
}

#[allow(renamed_and_removed_lints)]
mod proto;

mod macros;

#[cfg(feature = "global_register")]
#[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
mod global;

#[cfg(feature = "global_register")]
pub use global::*;
