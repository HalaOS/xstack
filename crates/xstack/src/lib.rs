//! **XSTACK** is developed with the primary goal of making it less difficult to use the [`libp2p`](https://libp2p.io/) network stack in rust.
//!
//! For this purpose, the API of **XSTACK** has been designed to closely resemble the [`std::net`](https://doc.rust-lang.org/std/net/index.html) library.
//!
//! ## Connect to peer and negotiate the application layer protocol
//!
//! ```no_run
//! use xstack::ProtocolStream;
//!
//! # async fn boostrap() {
//! let (stream, negotiated_proto_id) = ProtocolStream::connect(
//!     "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
//!     ["/ipfs/kad/1.0.0"]
//! ).await.unwrap();
//! # }
//! ```
//!
//! * The first parameter of [`connect`](ProtocolStream::connect) is a [`multiaddr`](https://github.com/multiformats/multiaddr) string,
//! indicates that the network stack shall create/reuse a `QUIC` transport connection to peer `QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ`;
//! In order to conserve resources, the protocol stack may not follow the multiaddr recommendation,
//! but instead directly reuse a pre-existing connection for a different transport layer protocol;
//!
//! * The second parameter is a list of candidate protocols, the network protocol stack will use this list to negotiate with peer to
//! select a protocol to communicate with.
//!
//! ## Server-side listeners.
//!
//! There are two types of communication listeners
//!
//! ### ***one for the transport layer***
//!
//! If the node needs to be able to receive inbound transport connections, developers need to manually create transport listeners;
//! the **transport listener** is more like a traditional socket listener, but developers can't handle inbound connections manually,
//! that part is handled automatically by the stack.
//!
//! the following codes creates an instance of the [`Switch`](**network stack**) and binds a tcp transport listener to it:
//!
//!
//! ```no_run
//! use xstack::Switch;
//!
//! # async fn boostrap() {
//! Switch::new("test")
//!       // .transport(TcpTransport)
//!       .transport_bind(["/ip4/127.0.0.1/tcp/0"])
//!       .create()
//!       .await
//!       .unwrap()
//!       // register to global context.
//!       .into_global();
//! # }
//! ```
//!
//! ### ***one for the application layer***
//!
//! If a node wishes to process a specific rpc request from a peer node, it should create a corresponding **protocol listener**;
//!
//! use a list of candidate protocols as the parameter to create one:
//!
//! ```no_run
//! use xstack::ProtocolListener;
//! use futures::TryStreamExt;
//!
//! # async fn boostrap() {
//! let mut incoming = ProtocolListener::bind(["/ipfs/kad/1.0.0"]).await.unwrap().into_incoming();
//!
//! while let Some((stream,_)) = incoming.try_next().await.unwrap() {
//!     // handle rpc request.
//! }
//! # }
//! ```
//!
//! ## Modularity
//!
//! **XSTACK** is designed to be modular, allowing developers to mix and match different components to meet the needs of
//! their particular application. This makes it easy to customize the networking stack to fit the specific requirements
//! of any P2P application.
//!
//! There are two types of components to customize the networking stack
//!
//! * ***Transport***: xstack provides a set of [`traits`](transport_syscall) that can be adapted to support various transport protocols,
//! allowing xstack applications to operate in various networking environments as the wealth of transport
//! protocol choices makes it possible to use xstack in a variety of scenarios.
//!
//! * ***Protocol***: based on [`ProtocolStream`]/[`ProtocolListener`], developers can customise the application layer/business logic.
//! implement utilities such as: node routing, data storage, overlay networks, etc.
//!
//!
//! ### Customize networking stack

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
