//! A [***Circuit Relay v2***] protocol implementation
//!
//! [***Circuit Relay v2***]: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md

#![cfg_attr(docsrs, feature(doc_cfg))]

mod proto;

mod rpc;
pub use rpc::*;

mod transport;
pub use transport::*;

mod errors;
pub use errors::*;

mod protocols;
pub use protocols::*;

/// Protocol id for `stop protocol`.
pub const PROTOCOL_CIRCUIT_RELAY_STOP: &str = "/libp2p/circuit/relay/0.2.0/stop";

/// Protocol id for `hop protocol`.
pub const PROTOCOL_CIRCUIT_RELAY_HOP: &str = "/libp2p/circuit/relay/0.2.0/hop";
