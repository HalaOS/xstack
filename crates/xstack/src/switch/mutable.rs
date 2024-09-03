use std::collections::{HashMap, VecDeque};

use multiaddr::Multiaddr;

use crate::{
    event::{Event, EventArgument, EventMediator, EventSource},
    transport::ProtocolStream,
    Error, Result, TransportConnection,
};

use super::{AutoNAT, ListenerId, PROTOCOL_IPFS_ID, PROTOCOL_IPFS_PING, PROTOCOL_IPFS_PUSH_ID};

#[derive(Default)]
pub(super) struct MutableSwitch {
    early_inbound_stream_cached_size: usize,
    inbound_streams: HashMap<ListenerId, VecDeque<(ProtocolStream, String)>>,
    laddrs: Vec<Multiaddr>,
    nat_addrs: Vec<Multiaddr>,
    protos: HashMap<String, ListenerId>,
    event_mediator: EventMediator,
    unauth_inbound_streams: HashMap<String, Vec<(ProtocolStream, String)>>,
    nat: AutoNAT,
}

impl MutableSwitch {
    pub(super) fn new(early_inbound_stream_cached_size: usize) -> Self {
        Self {
            early_inbound_stream_cached_size,
            ..Default::default()
        }
    }

    /// Register transport bind addresses.
    pub(super) fn transport_bind_to(&mut self, addr: Multiaddr) {
        self.laddrs.push(addr)
    }

    /// Returns the local bound addrs.
    pub(super) fn local_addrs(&self) -> Vec<Multiaddr> {
        self.laddrs.clone()
    }

    pub(super) fn listen_addrs(&self) -> Vec<Multiaddr> {
        if self.nat == AutoNAT::NAT {
            self.nat_addrs.clone()
        } else {
            self.laddrs.clone()
        }
    }

    pub(super) fn set_net_addrs(&mut self, addrs: Vec<Multiaddr>) {
        self.nat_addrs = addrs;
    }

    /// Create a new server-side socket that accept inbound protocol stream.
    pub(super) fn new_protocol_listener<I>(&mut self, protos: I) -> Result<ListenerId>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let id = ListenerId::next();

        for proto in protos.into_iter() {
            if self.protos.contains_key(proto.as_ref()) {
                return Err(Error::BindError(proto.as_ref().to_owned()));
            }

            if self.protos.insert(proto.as_ref().to_owned(), id).is_some() {
                return Err(Error::ProtocolListenerBind(proto.as_ref().to_owned()));
            }
        }

        assert!(
            self.inbound_streams
                .insert(id, VecDeque::default())
                .is_none(),
            "The `ListenerId` cannot be duplicated."
        );

        Ok(id)
    }

    /// Close the listener by id, and remove queued inbound stream.
    pub(super) fn close_protocol_listener(&mut self, id: &ListenerId) {
        let keys = self
            .protos
            .iter()
            .filter_map(|(proto, value)| {
                if *value == *id {
                    Some(proto.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for key in keys {
            self.protos.remove(&key);
        }

        self.inbound_streams.remove(id);
    }

    /// Pop one inbound stream from one listener's incoming queue.
    pub(super) fn incoming_next(
        &mut self,
        id: &ListenerId,
    ) -> Result<Option<(ProtocolStream, String)>> {
        Ok(self
            .inbound_streams
            .get_mut(id)
            .ok_or(Error::ProtocolListener(id.into()))?
            .pop_front())
    }

    /// Insert a new inbound stream.
    ///
    /// the protocol listener does not exist or has been closed, it is simply dropped.
    pub(super) fn insert_inbound_stream(
        &mut self,
        stream: ProtocolStream,
        proto: String,
    ) -> Option<ListenerId> {
        if self.unauth_inbound_streams.contains_key(stream.conn_id()) {
            self.insert_unauth_inbound_stream(stream, proto);
            return None;
        }

        if let Some(id) = self.protos.get(&proto) {
            self.inbound_streams
                .get_mut(id)
                .expect("Atomic cleanup listener resources guarantee")
                .push_back((stream, proto));

            Some(*id)
        } else {
            log::warn!("Protocol listener is not exists, {}", proto);
            None
        }
    }

    pub(super) fn start_conn_handshake(&mut self, conn: &TransportConnection) {
        self.unauth_inbound_streams
            .insert(conn.id().to_owned(), vec![]);
    }

    pub(super) fn conn_handshake_failed(&mut self, conn: &TransportConnection) {
        self.unauth_inbound_streams.remove(conn.id());
        self.unauth_inbound_streams
            .insert(conn.id().to_owned(), vec![]);
    }

    /// Put a new connecton instance into the pool, and update indexers.
    pub(super) fn conn_handshake_succ(&mut self, conn: TransportConnection) {
        if let Some(streams) = self.unauth_inbound_streams.remove(conn.id()) {
            for (stream, proto) in streams {
                self.insert_inbound_stream(stream, proto);
            }
        }
    }
    /// Insert a new inbound stream from an unauthenticated connection.
    ///
    /// the protocol listener does not exist or has been closed, it is simply dropped.
    pub(super) fn insert_unauth_inbound_stream(&mut self, stream: ProtocolStream, proto: String) {
        if let Some(streams) = self.unauth_inbound_streams.get_mut(stream.conn_id()) {
            if streams.len() > self.early_inbound_stream_cached_size {
                log::warn!(
                    "early inbound stream limits reached, conn={}, stream={}",
                    stream.conn_id(),
                    stream.id()
                );
                return;
            }

            streams.push((stream, proto));
        } else {
            self.unauth_inbound_streams
                .insert(stream.conn_id().to_owned(), vec![(stream, proto)]);
        }
    }

    pub(super) fn protos(&self) -> Vec<String> {
        let mut protos = vec![
            PROTOCOL_IPFS_ID.to_owned(),
            PROTOCOL_IPFS_PING.to_owned(),
            PROTOCOL_IPFS_PUSH_ID.to_owned(),
        ];

        for key in self.protos.keys() {
            protos.push(key.to_owned());
        }

        protos
    }

    pub(super) fn notify(&mut self, arg: EventArgument) {
        self.event_mediator.notify(arg)
    }

    pub(super) fn new_listener<E: Event>(&mut self, buffer: usize) -> EventSource<E> {
        self.event_mediator.new_listener(buffer)
    }

    pub(super) fn auto_nat(&self) -> AutoNAT {
        self.nat
    }

    pub(super) fn set_nat(&mut self, state: AutoNAT) {
        if self.nat != state {
            self.notify(EventArgument::AutoNAT(state));
        }

        self.nat = state;
    }
}
