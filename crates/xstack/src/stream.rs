use std::{
    collections::{HashMap, VecDeque},
    io::Result,
    sync::Arc,
};

use async_trait::async_trait;
use futures::{lock::Mutex, stream::unfold, Stream};
use futures_map::KeyWaitMap;
use rasi::task::spawn_ok;

use crate::{driver_wrapper, global_switch, Error, ProtocolStream, Switch, XStackId};

/// A libp2p protocol dispather driver must implement the `Driver-*` traits in this module.
pub mod stream_syscall {
    use std::io::Result;

    use async_trait::async_trait;

    use crate::ProtocolStream;

    use super::XStackId;

    #[async_trait]
    pub trait DriverStreamDispatcher: Sync + Send {
        async fn bind(&self, id: XStackId, protocols: &[String]) -> Result<()>;

        async fn close(&self, id: XStackId);

        async fn accept(&self, id: XStackId) -> Result<(ProtocolStream, String)>;

        async fn handshake(&self, conn_id: &str);

        async fn handshake_success(&self, conn_id: &str);
        async fn handshake_failed(&self, conn_id: &str);

        async fn dispatch(&self, stream: ProtocolStream, protocol_id: String);

        async fn protos(&self) -> Vec<String>;
    }
}

driver_wrapper!(
    ["A type wrapper of [`DriverStreamDispatcher`](stream_syscall::DriverStreamDispatcher)"]
    StreamDispatcher[stream_syscall::DriverStreamDispatcher]
);

/// A listener of protocol streams.
pub struct ProtocolListener {
    switch: Switch,
    id: XStackId,
}

impl ProtocolListener {
    pub fn new(switch: Switch, id: XStackId) -> Self {
        Self { switch, id }
    }

    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub async fn bind<I>(protocols: I) -> crate::Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        Self::bind_with(global_switch(), protocols).await
    }
    pub async fn bind_with<I>(switch: &Switch, protocols: I) -> crate::Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        switch.bind(protocols).await
    }

    /// Create a listener closer.
    pub fn to_closer(&self) -> ProtocolListenerCloser {
        ProtocolListenerCloser {
            switch: self.switch.clone(),
            id: self.id,
        }
    }

    pub async fn accept(&self) -> Result<(ProtocolStream, String)> {
        self.switch.stream_dispatcher().accept(self.id).await
    }

    pub fn into_incoming(self) -> impl Stream<Item = Result<(ProtocolStream, String)>> + Unpin {
        Box::pin(unfold(self, |listener| async move {
            let res = listener.accept().await;
            Some((res, listener))
        }))
    }
}

impl Drop for ProtocolListener {
    fn drop(&mut self) {
        let closer = self.to_closer();

        spawn_ok(async move {
            closer.close().await;
        })
    }
}

pub struct ProtocolListenerCloser {
    switch: Switch,
    id: XStackId,
}

impl ProtocolListenerCloser {
    pub async fn close(&self) {
        self.switch.stream_dispatcher().close(self.id).await;
    }
}

#[derive(Default)]
struct RawMutexStreamDispatcher {
    is_closed: bool,
    inbound_streams: HashMap<XStackId, VecDeque<(ProtocolStream, String)>>,
    protos: HashMap<String, XStackId>,
    unauth_streams: HashMap<String, Vec<(ProtocolStream, String)>>,
}

/// The default implementation of [`StreamDispatcher`]
#[derive(Default)]
pub struct MutexStreamDispatcher {
    raw: Arc<Mutex<RawMutexStreamDispatcher>>,
    event_map: Arc<KeyWaitMap<XStackId, ()>>,
}

impl Drop for MutexStreamDispatcher {
    fn drop(&mut self) {
        let raw = self.raw.clone();
        let event_map = self.event_map.clone();
        spawn_ok(async move {
            raw.lock().await.is_closed = true;
            event_map.cancel_all();
        });
    }
}

#[async_trait]
impl stream_syscall::DriverStreamDispatcher for MutexStreamDispatcher {
    async fn handshake(&self, conn_id: &str) {
        self.raw
            .lock()
            .await
            .unauth_streams
            .insert(conn_id.to_owned(), vec![]);
    }

    async fn handshake_success(&self, conn_id: &str) {
        let streams = self.raw.lock().await.unauth_streams.remove(conn_id);

        if let Some(streams) = streams {
            for (stream, protocol_id) in streams {
                self.dispatch(stream, protocol_id).await;
            }
        }
    }
    async fn handshake_failed(&self, conn_id: &str) {
        let streams = self.raw.lock().await.unauth_streams.remove(conn_id);

        if let Some(streams) = streams {
            for (stream, protocol_id) in streams {
                log::warn!(
                    "drop stream from={} protocol_id={}",
                    stream.peer_addr(),
                    protocol_id
                );
            }
        }
    }
    async fn bind(&self, id: XStackId, protocols: &[String]) -> Result<()> {
        let mut raw = self.raw.lock().await;

        if raw.is_closed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "dispatcher is drainning",
            ));
        }

        for proto in protocols {
            if raw.protos.contains_key(proto) {
                return Err(Error::ProtocolListenerBind(proto.to_owned()).into());
            }
        }

        for proto in protocols {
            raw.protos.insert(proto.to_owned(), id);
        }

        raw.inbound_streams.insert(id, Default::default());

        Ok(())
    }

    async fn close(&self, id: XStackId) {
        let mut raw = self.raw.lock().await;

        let keys = raw
            .protos
            .iter()
            .filter_map(|(proto, value)| {
                if *value == id {
                    Some(proto.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for key in keys {
            raw.protos.remove(&key);
        }

        raw.inbound_streams.remove(&id);

        drop(raw);

        self.event_map.insert(id, ());
    }

    async fn accept(&self, id: XStackId) -> Result<(ProtocolStream, String)> {
        loop {
            let mut raw = self.raw.lock().await;

            if let Some(streams) = raw.inbound_streams.get_mut(&id) {
                if let Some(stream) = streams.pop_front() {
                    return Ok(stream);
                }
            } else {
                return Err(Error::ProtocolListener(id.0).into());
            }

            self.event_map.wait(&id, raw).await;
        }
    }

    async fn dispatch(&self, stream: ProtocolStream, protocol_id: String) {
        let mut raw = self.raw.lock().await;

        if let Some(cached) = raw.unauth_streams.get_mut(stream.conn_id()) {
            cached.push((stream, protocol_id));
            return;
        }

        if let Some(id) = raw.protos.get(&protocol_id).map(|id| id.clone()) {
            raw.inbound_streams
                .get_mut(&id)
                .expect("Consistency Guarantee")
                .push_back((stream, protocol_id));

            drop(raw);

            self.event_map.insert(id, ());
        } else {
            log::warn!(
                "drop stream from={}, unhandled protocol={}",
                stream.peer_addr(),
                protocol_id
            );
        }
    }

    async fn protos(&self) -> Vec<String> {
        self.raw.lock().await.protos.keys().cloned().collect()
    }
}
