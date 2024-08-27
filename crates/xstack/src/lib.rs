#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod book;
pub mod keystore;
pub mod multiaddr;

pub mod transport;

mod switch;
pub use switch::*;

mod errors;
pub use errors::*;

pub use identity;

#[allow(renamed_and_removed_lints)]
mod proto;

mod macros;

#[cfg(feature = "global_register")]
#[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
mod global;

#[cfg(feature = "global_register")]
pub use global::*;
