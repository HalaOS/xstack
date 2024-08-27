use std::{sync::Arc, time::Duration};

use futures::lock::Mutex;
use futures_map::KeyWaitMap;
use multiaddr::Multiaddr;

use crate::{
    book::{syscall::DriverPeerBook, MemoryPeerBook, PeerBook},
    keystore::{syscall::DriverKeyStore, KeyStore, MemoryKeyStore},
    transport::{syscall::DriverTransport, Transport},
    Error, Result,
};

use super::{mutable::MutableSwitch, InnerSwitch, Switch};

/// immutable context data for one switch.
pub(super) struct ImmutableSwitch {
    /// The maximun size of active connections pool size.
    pub(super) max_conn_pool_size: usize,
    /// The value of rpc timeout.
    pub(super) timeout: Duration,
    /// This is a free-form string, identitying the implementation of the peer. The usual format is agent-name/version,
    /// where agent-name is the name of the program or library and version is its semantic version.
    pub(super) agent_version: String,
    /// The max length of identity packet.
    pub(super) max_identity_packet_size: usize,
    /// A list of transport that this switch registered.
    pub(super) transports: Vec<Transport>,
    /// Keystore registered to this switch.
    pub(super) keystore: KeyStore,
    /// Peer book for this switch.
    pub(super) peer_book: PeerBook,
}

impl ImmutableSwitch {
    pub(super) fn new(agent_version: String) -> Self {
        Self {
            max_conn_pool_size: 20,
            agent_version,
            timeout: Duration::from_secs(10),
            max_identity_packet_size: 4096,
            transports: vec![],
            keystore: MemoryKeyStore::random().into(),
            peer_book: MemoryPeerBook::default().into(),
        }
    }

    pub(super) fn get_transport_by_address(&self, laddr: &Multiaddr) -> Option<&Transport> {
        self.transports
            .iter()
            .find(|transport| transport.multiaddr_hit(laddr))
    }
}

struct SwitchBuilderInner {
    laddrs: Vec<Multiaddr>,
    immutable: ImmutableSwitch,
}

/// A builder to create the `Switch` instance.
pub struct SwitchBuilder {
    ops: Result<SwitchBuilderInner>,
}

impl SwitchBuilder {
    pub(super) fn new(agent_version: String) -> Self {
        Self {
            ops: Ok(SwitchBuilderInner {
                laddrs: Default::default(),
                immutable: ImmutableSwitch::new(agent_version),
            }),
        }
    }
    /// Set the `max_conn_pool_size`, the default value is `20`
    pub fn max_conn_pool_size(self, value: usize) -> Self {
        self.and_then(|mut cfg| {
            cfg.immutable.max_conn_pool_size = value;

            Ok(cfg)
        })
    }

    /// Replace default [`MemoryKeyStore`].
    pub fn keystore<K>(self, value: K) -> Self
    where
        K: DriverKeyStore + 'static,
    {
        self.and_then(|mut cfg| {
            cfg.immutable.keystore = value.into();

            Ok(cfg)
        })
    }

    /// Replace default [`MemoryPeerBook`].
    pub fn peer_book<R>(self, value: R) -> Self
    where
        R: DriverPeerBook + 'static,
    {
        self.and_then(|mut cfg| {
            cfg.immutable.peer_book = value.into();

            Ok(cfg)
        })
    }

    /// Set the protocol timeout, the default value is `10s`
    pub fn timeout(self, duration: Duration) -> Self {
        self.and_then(|mut cfg| {
            cfg.immutable.timeout = duration;

            Ok(cfg)
        })
    }

    /// Set the receive max buffer length of identity protocol.
    pub fn max_identity_packet_size(self, value: usize) -> Self {
        self.and_then(|mut cfg| {
            cfg.immutable.max_identity_packet_size = value;

            Ok(cfg)
        })
    }

    /// Register a new transport driver for the switch.
    pub fn transport<T>(self, value: T) -> Self
    where
        T: DriverTransport + 'static,
    {
        self.and_then(|mut cfg| {
            cfg.immutable.transports.push(value.into());

            Ok(cfg)
        })
    }

    pub fn transport_bind<I, E>(self, laddrs: I) -> Self
    where
        I: IntoIterator,
        I::Item: TryInto<Multiaddr, Error = E>,
        Error: From<E>,
    {
        self.and_then(|mut cfg| {
            cfg.laddrs = laddrs
                .into_iter()
                .map(|item| item.try_into().map_err(|err| err.into()))
                .collect::<Result<Vec<Multiaddr>>>()?;

            Ok(cfg)
        })
    }

    /// Consume the builder and create a new `Switch` instance.
    pub async fn create(self) -> Result<Switch> {
        let ops = self.ops?;

        let public_key = ops.immutable.keystore.public_key().await?;

        let switch = Switch {
            inner: Arc::new(InnerSwitch {
                local_peer_id: public_key.to_peer_id(),
                public_key,
                mutable: Mutex::new(MutableSwitch::new(ops.immutable.max_conn_pool_size)),
                immutable: ops.immutable,
                event_map: KeyWaitMap::new(),
            }),
        };

        for laddr in ops.laddrs {
            switch.transport_bind(&laddr).await?;
        }

        Ok(switch)
    }

    fn and_then<F>(self, func: F) -> Self
    where
        F: FnOnce(SwitchBuilderInner) -> Result<SwitchBuilderInner>,
    {
        SwitchBuilder {
            ops: self.ops.and_then(func),
        }
    }
}
