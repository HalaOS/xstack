use std::collections::{HashMap, VecDeque};

use multiaddr::Multiaddr;

use crate::{transport::ProtocolStream, Error, Result};

use super::{
    pool::ConnPool, ListenerId, PROTOCOL_IPFS_ID, PROTOCOL_IPFS_PING, PROTOCOL_IPFS_PUSH_ID,
};

#[derive(Default)]
pub(super) struct MutableSwitch {
    pub(super) conn_pool: ConnPool,
    incoming_streams: HashMap<ListenerId, VecDeque<(ProtocolStream, String)>>,
    laddrs: Vec<Multiaddr>,
    protos: HashMap<String, ListenerId>,
}

impl MutableSwitch {
    pub(super) fn new(max_pool_size: usize) -> Self {
        Self {
            conn_pool: ConnPool::new(max_pool_size),
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

            self.protos.insert(proto.as_ref().to_owned(), id);
        }

        assert!(
            self.incoming_streams
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

        self.incoming_streams.remove(id);
    }

    /// Pop one inbound stream from one listener's incoming queue.
    pub(super) fn incoming_next(
        &mut self,
        id: &ListenerId,
    ) -> Result<Option<(ProtocolStream, String)>> {
        Ok(self
            .incoming_streams
            .get_mut(id)
            .ok_or(Error::ProtocolListener(id.0))?
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
        if let Some(id) = self.protos.get(&proto) {
            self.incoming_streams
                .get_mut(id)
                .expect("Atomic cleanup listener resources guarantee")
                .push_back((stream, proto));

            Some(*id)
        } else {
            log::warn!("Protocol listener is not exists, {}", proto);
            None
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
}
