use std::{sync::Arc, time::Duration};

use multiaddr::Multiaddr;

use crate::{
    book::{peerbook_syscall::DriverPeerBook, MemoryPeerBook, PeerBook},
    connector_syscall::DriverConnector,
    event_syscall::DriverEventMediator,
    keystore::{keystore_syscall::DriverKeyStore, KeyStore, MemoryKeyStore},
    stream_syscall::DriverStreamDispatcher,
    transport::{transport_syscall::DriverTransport, Transport},
    ConnPool, Connector, Error, EventMediator, MutexStreamDispatcher, Result, StreamDispatcher,
    SyncEventMediator,
};

use super::Switch;

/// immutable context data for one switch.
pub(super) struct ImmutableSwitch {
    /// The value of rpc timeout.
    pub(super) timeout: Duration,
    /// This is a free-form string, identitying the implementation of the peer. The usual format is agent-name/version,
    /// where agent-name is the name of the program or library and version is its semantic version.
    pub(super) agent_version: String,
    /// Maximun length of libp2p rpc packets.
    pub(super) max_packet_size: usize,
    /// A list of transport that this switch registered.
    pub(super) transports: Vec<Transport>,
    /// Keystore registered to this switch.
    pub(super) keystore: KeyStore,
    /// Peer book for this switch.
    pub(super) peer_book: PeerBook,
    /// Connector for this switch.
    pub(super) connector: Connector,
    /// StreamDispatcher for this switch.
    pub(super) stream_dispatcher: StreamDispatcher,
    /// EventMediator for this switch,
    pub(super) event_mediator: EventMediator,
}

impl ImmutableSwitch {
    pub(super) fn new(agent_version: String) -> Self {
        Self {
            agent_version,
            timeout: Duration::from_secs(5),
            max_packet_size: 1024 * 1024 * 4,
            transports: vec![],
            keystore: MemoryKeyStore::random().into(),
            peer_book: MemoryPeerBook::default().into(),
            connector: ConnPool::default().into(),
            stream_dispatcher: MutexStreamDispatcher::default().into(),
            event_mediator: SyncEventMediator::default().into(),
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
    early_inbound_stream_cached_size: usize,
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
                early_inbound_stream_cached_size: 10,
                immutable: ImmutableSwitch::new(agent_version),
            }),
        }
    }
    /// /// Replace default [`ConnPool`].
    pub fn connector<C>(self, value: C) -> Self
    where
        C: DriverConnector + 'static,
    {
        self.and_then(|mut cfg| {
            cfg.immutable.connector = value.into();

            Ok(cfg)
        })
    }

    /// Set the `early_inbound_stream_cached_size`, the default value is `10`.
    ///
    /// This parameter limits the early inbound stream cache queue size.
    pub fn early_inbound_stream_cached_size(self, value: usize) -> Self {
        self.and_then(|mut cfg| {
            cfg.early_inbound_stream_cached_size = value;

            Ok(cfg)
        })
    }

    /// Replace default [`SyncEventMediator`].
    pub fn event_mediator<K>(self, value: K) -> Self
    where
        K: DriverEventMediator + 'static,
    {
        self.and_then(|mut cfg| {
            cfg.immutable.event_mediator = value.into();

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

    /// Replace default [`MutexStreamDispatcher`].
    pub fn stream_dispatcher<K>(self, value: K) -> Self
    where
        K: DriverStreamDispatcher + 'static,
    {
        self.and_then(|mut cfg| {
            cfg.immutable.stream_dispatcher = value.into();

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

    /// Set the maximum length of incoming rpc packets. the default value is `1024 * 1024 * 4`
    pub fn max_packet_size(self, value: usize) -> Self {
        self.and_then(|mut cfg| {
            cfg.immutable.max_packet_size = value;

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

        if ops.immutable.transports.is_empty() {
            return Err(Error::NullTransportStack);
        }

        let public_key = ops.immutable.keystore.public_key().await?;

        let switch = Switch {
            local_peer_id: Arc::new(public_key.to_peer_id()),
            public_key: Arc::new(public_key),
            mutable: Default::default(),
            immutable: Arc::new(ops.immutable),
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
