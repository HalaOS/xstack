//! This module provides event/listener pattern support for `switch`.

use std::{collections::HashMap, marker::PhantomData, task::Poll};

use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    Stream, StreamExt,
};
use libp2p_identity::PeerId;

use crate::{AutoNAT, Switch};

/// A trait which switch event type must implement.
pub trait Event {
    /// Type of event associated value.
    type Argument;

    /// Returns event name.
    fn name() -> &'static str;

    /// Convert [`EventArgument`] into inner value.
    fn to_argument(arg: EventArgument) -> Self::Argument;
}

/// The variant of switch event argument
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventArgument {
    /// A inbound/outbound connection is established.
    Connected(PeerId),

    /// autonat state changed.
    AutoNAT(AutoNAT),
}

/// A [`Stream`] of event `E`.
pub struct EventSource<E> {
    receiver: Receiver<EventArgument>,
    _marker: PhantomData<E>,
}

impl<E> EventSource<E>
where
    E: Event,
{
    /// Bind a new `EventSource` to **global Switch Context**.
    ///
    /// # Parameters
    ///
    /// - `buffer`, the size of the innner queue to cache the events.
    ///
    ///
    /// ***This function internally calls [`global_switch`](crate::global_switch) to get [`Switch`] instance,
    /// so calling this function before calling [`register_switch`](crate::register_switch) will cause panic.***
    ///

    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub async fn bind(buffer: usize) -> Self {
        use crate::global_switch;

        global_switch().on(buffer).await
    }

    /// Bind a new `EventSource` to `switch`, refer to [`bind`](Self::bind) for more details.
    pub async fn bind_with(switch: &Switch, buffer: usize) -> Self {
        switch.on(buffer).await
    }
}

impl<E> Stream for EventSource<E>
where
    E: Event + Unpin,
{
    type Item = E::Argument;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.receiver.poll_next_unpin(cx) {
            std::task::Poll::Ready(argument) => Poll::Ready(argument.map(E::to_argument)),
            std::task::Poll::Pending => Poll::Pending,
        }
    }
}

/// listenable events of `Switch`.
pub mod events {
    use crate::AutoNAT;

    use super::*;
    /// Peer connection event
    pub struct Connected;

    impl Event for Connected {
        type Argument = PeerId;

        fn name() -> &'static str {
            "/xstack/event/connected/1.0.0"
        }

        fn to_argument(arg: EventArgument) -> Self::Argument {
            match arg {
                EventArgument::Connected(peer_id) => peer_id,
                _ => panic!("not here"),
            }
        }
    }

    /// [*autonat protocol*](https://github.com/libp2p/specs/tree/master/autonat) state changed event.
    pub struct AutoNATChanged;

    impl Event for AutoNATChanged {
        type Argument = AutoNAT;

        fn name() -> &'static str {
            "/xstack/event/autonat/1.0.0"
        }

        fn to_argument(arg: EventArgument) -> Self::Argument {
            match arg {
                EventArgument::AutoNAT(auto_nat) => auto_nat,
                _ => panic!("not here"),
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct EventMediator(HashMap<String, Vec<Sender<EventArgument>>>);

impl EventMediator {
    pub(crate) fn notify(&mut self, arg: EventArgument) {
        match &arg {
            EventArgument::Connected(_) => {
                self.notify_inner::<events::Connected>(arg);
            }
            EventArgument::AutoNAT(_) => self.notify_inner::<events::AutoNATChanged>(arg),
        }
    }

    fn notify_inner<E>(&mut self, arg: EventArgument)
    where
        E: Event,
    {
        if let Some(senders) = self.0.remove(E::name()) {
            let mut valid_senders = vec![];

            for mut sender in senders {
                if let Err(err) = sender.try_send(arg.clone()) {
                    if err.is_disconnected() {
                        log::trace!("remove closed connected event listener");
                        continue;
                    }
                }

                valid_senders.push(sender);
            }

            self.0
                .insert(events::Connected::name().to_owned(), valid_senders);
        }
    }

    pub(crate) fn new_listener<E: Event>(&mut self, buffer: usize) -> EventSource<E> {
        let (sender, receiver) = channel(buffer);
        if let Some(senders) = self.0.get_mut(E::name()) {
            senders.push(sender);
        } else {
            self.0.insert(E::name().to_owned(), vec![sender]);
        }

        EventSource {
            receiver,
            _marker: Default::default(),
        }
    }
}
