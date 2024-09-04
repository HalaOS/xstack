use std::{collections::HashMap, marker::PhantomData, num::NonZeroUsize, task::Poll};

use async_trait::async_trait;
use futures::{
    channel::mpsc::{channel, Sender},
    lock::Mutex,
    SinkExt, Stream, StreamExt,
};
use libp2p_identity::PeerId;
use rasi::task::spawn_ok;

use crate::{driver_wrapper, AutoNAT, Switch, XStackId};

/// A xstack event mediator driver must implement the `Driver-*` traits in this module.
pub mod event_syscall {

    use std::num::NonZeroUsize;

    use async_trait::async_trait;
    use futures::Stream;

    use crate::XStackId;

    use super::{Event, EventStream};

    #[async_trait]
    pub trait DriverEventMediator: Sync + Send {
        /// Create new event receiver.
        async fn bind(&self, id: XStackId, event_name: &str, buffer: NonZeroUsize) -> EventStream;

        /// Close the event stream by id.
        async fn close(&self, id: &XStackId);

        /// Dispatch the event to the register listeners.
        async fn raise(&self, event: Event);
    }

    ///
    pub trait DriverEventStream: Stream<Item = Event> + Unpin {}

    impl<S> DriverEventStream for S where S: Stream<Item = Event> + Unpin {}
}

driver_wrapper!(
    ["A type wrapper of [`EventMediator`](event_syscall::DriverEventMediator)"]
    EventMediator[event_syscall::DriverEventMediator]
);

driver_wrapper!(
    ["A type wrapper of [`EventStream`](event_syscall::DriverEventStream)"]
    EventStream[event_syscall::DriverEventStream]
);

static EVENT_CONNECTED: &str = "/xstack/connected";
static EVENT_HANDSHAKE_SUCCESS: &str = "/xstack/handshake_success";
static EVENT_HANDSHAKE_FAILED: &str = "/xstack/handshake_failed";
static EVENT_NETWORK: &str = "/xstack/network";

/// A `Switch` event must implement this trait.
pub trait ToEventArgument {
    /// Event argument type.
    type Argument;

    fn name() -> &'static str;
    fn into_argument(event: Event) -> Self::Argument;
}

/// Variant of switch events.
#[derive(Clone)]
pub enum Event {
    /// The connection state changed.
    Connected {
        conn_id: String,
        peer_id: PeerId,
    },

    HandshakeSuccess {
        conn_id: String,
        peer_id: PeerId,
    },

    HandshakeFailed {
        conn_id: String,
        peer_id: PeerId,
    },

    /// The network state changed.
    Network(AutoNAT),
}

impl Event {
    /// Get the event name.
    pub fn to_name(&self) -> &'static str {
        match self {
            Event::Network(_) => &EVENT_NETWORK,
            Event::Connected {
                conn_id: _,
                peer_id: _,
            } => &EVENT_CONNECTED,
            Event::HandshakeSuccess {
                conn_id: _,
                peer_id: _,
            } => &EVENT_HANDSHAKE_SUCCESS,
            Event::HandshakeFailed {
                conn_id: _,
                peer_id: _,
            } => &EVENT_HANDSHAKE_FAILED,
        }
    }
}

/// events of switch.
pub mod events {
    use libp2p_identity::PeerId;

    use crate::AutoNAT;

    use super::{
        Event, ToEventArgument, EVENT_CONNECTED, EVENT_HANDSHAKE_FAILED, EVENT_HANDSHAKE_SUCCESS,
        EVENT_NETWORK,
    };

    pub struct Connected;

    impl ToEventArgument for Connected {
        type Argument = (String, PeerId);

        fn name() -> &'static str {
            &EVENT_CONNECTED
        }

        fn into_argument(event: super::Event) -> Self::Argument {
            match event {
                Event::Connected { conn_id, peer_id } => (conn_id, peer_id),
                _ => panic!("not here"),
            }
        }
    }

    pub struct HandshakeSuccess;

    impl ToEventArgument for HandshakeSuccess {
        type Argument = (String, PeerId);

        fn name() -> &'static str {
            &EVENT_HANDSHAKE_SUCCESS
        }

        fn into_argument(event: super::Event) -> Self::Argument {
            match event {
                Event::HandshakeSuccess { conn_id, peer_id } => (conn_id, peer_id),
                _ => panic!("not here"),
            }
        }
    }

    pub struct HandshakeFailed;

    impl ToEventArgument for HandshakeFailed {
        type Argument = (String, PeerId);

        fn name() -> &'static str {
            &EVENT_HANDSHAKE_FAILED
        }

        fn into_argument(event: super::Event) -> Self::Argument {
            match event {
                Event::HandshakeFailed { conn_id, peer_id } => (conn_id, peer_id),
                _ => panic!("not here"),
            }
        }
    }

    pub struct Network;

    impl ToEventArgument for Network {
        type Argument = AutoNAT;

        fn name() -> &'static str {
            &EVENT_NETWORK
        }

        fn into_argument(event: super::Event) -> Self::Argument {
            match event {
                Event::Network(value) => value,
                _ => panic!("not here"),
            }
        }
    }
}

pub struct EventSourceCloser {
    id: XStackId,
    switch: Switch,
}

impl EventSourceCloser {
    pub async fn close(&self) {
        self.switch.event_mediator.close(&self.id).await
    }
}

/// A [`stream`](futures::Stream) that accept a kined of event `E`
pub struct EventSource<E> {
    id: XStackId,
    switch: Switch,
    stream: EventStream,
    _marker: PhantomData<E>,
}

impl<E> Drop for EventSource<E> {
    fn drop(&mut self) {
        let closer = self.to_closer();

        spawn_ok(async move { closer.close().await })
    }
}

impl<E> Stream for EventSource<E>
where
    E: ToEventArgument + Unpin,
{
    type Item = E::Argument;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.stream.poll_next_unpin(cx) {
            std::task::Poll::Ready(argument) => Poll::Ready(argument.map(E::into_argument)),
            std::task::Poll::Pending => Poll::Pending,
        }
    }
}

impl<E> EventSource<E> {
    /// Create a new closer for this `EventSource`
    pub fn to_closer(&self) -> EventSourceCloser {
        EventSourceCloser {
            id: self.id,
            switch: self.switch.clone(),
        }
    }
}

impl<E> EventSource<E>
where
    E: ToEventArgument,
{
    /// Create a new `EventSource` and bind to provides `switch`.
    pub async fn bind_with(switch: &Switch, buffer: NonZeroUsize) -> Self {
        let id = XStackId::default();

        let stream = switch.event_mediator.bind(id, E::name(), buffer).await;

        Self {
            id,
            switch: switch.clone(),
            stream,
            _marker: Default::default(),
        }
    }

    /// Create a new `EventSource` and bind to global context `switch`.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub async fn bind(buffer: NonZeroUsize) -> Self {
        use crate::global_switch;

        Self::bind_with(global_switch(), buffer).await
    }
}

/// The default [`EventMediator`] implementation, it guarantees that event messages will not be lost.
#[derive(Default)]
pub struct SyncEventMediator(Mutex<HashMap<String, HashMap<XStackId, Sender<Event>>>>);

#[async_trait]
impl event_syscall::DriverEventMediator for SyncEventMediator {
    async fn bind(&self, id: XStackId, event_name: &str, buffer: NonZeroUsize) -> EventStream {
        let (sender, receiver) = channel(buffer.into());

        let mut raw = self.0.lock().await;

        if let Some(senders) = raw.get_mut(event_name) {
            senders.insert(id, sender);
        } else {
            let mut map = HashMap::new();
            map.insert(id, sender);
            raw.insert(event_name.to_owned(), map);
        }

        receiver.into()
    }

    async fn raise(&self, event: Event) {
        let senders = self
            .0
            .lock()
            .await
            .get(event.to_name())
            .map(|senders| senders.clone());

        if let Some(senders) = senders {
            let mut disconnected = vec![];

            for (id, mut sender) in senders {
                log::trace!("raise.... {}", id);
                if let Err(err) = sender.send(event.clone()).await {
                    log::warn!("dispatch event {} failed, {}", event.to_name(), err);
                    disconnected.push(id);
                }
            }

            let mut raw = self.0.lock().await;

            if let Some(map) = raw.get_mut(event.to_name()) {
                for id in disconnected {
                    map.remove(&id);
                }
            }
        }
    }

    async fn close(&self, id: &XStackId) {
        let mut raw = self.0.lock().await;

        for map in raw.values_mut() {
            if map.remove(id).is_some() {
                return;
            }
        }
    }
}
