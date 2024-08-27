#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod book;
pub mod keystore;
pub mod multiaddr;

mod transport;
pub use transport::*;

mod switch;
pub use switch::*;

mod errors;
pub use errors::*;

pub use libp2p_identity as identity;

#[allow(renamed_and_removed_lints)]
mod proto;

mod macros;

#[cfg(feature = "global_register")]
#[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
mod global;

#[cfg(feature = "global_register")]
pub use global::*;
