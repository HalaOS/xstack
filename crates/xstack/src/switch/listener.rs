use std::ops::Deref;

use rasi::task::spawn_ok;

use crate::{transport::ProtocolStream, Result};

use super::{ListenerId, Switch, SwitchInnerEvent};

#[derive(Clone)]
pub struct ProtocolListenerState {
    id: ListenerId,
    switch: Switch,
}

impl ProtocolListenerState {
    /// Close the listener.
    pub async fn close(&self) {
        self.switch
            .mutable
            .lock()
            .await
            .close_protocol_listener(&self.id);
    }

    /// Accept a new inbound [`ProtocolStream`].
    ///
    /// On success, returns the negotiated protocol id with the stream.
    pub async fn accept(&self) -> Result<(ProtocolStream, String)> {
        loop {
            if let Some(incoming) = self.switch.mutable.lock().await.incoming_next(&self.id)? {
                return Ok(incoming);
            }

            _ = self
                .switch
                .event_map
                .wait(&SwitchInnerEvent::Accept(self.id), ());
        }
    }
}

/// A server-side socket that accept new inbound [`ProtocolStream`]
pub struct ProtocolListener(ProtocolListenerState);

impl ProtocolListener {
    pub(super) fn new(id: ListenerId, switch: Switch) -> Self {
        Self(ProtocolListenerState { id, switch })
    }

    /// Create a new `ProtocolListener` with `protos`.
    ///
    /// This function internally calls [`global_switch`](crate::global_switch) to get [`Switch`] instance,
    /// so calling this function before calling [`register_switch`](crate::register_switch) will cause panic.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub async fn bind<I>(protos: I) -> Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        use crate::global_switch;

        Self::bind_with(global_switch(), protos).await
    }

    /// Create a new `ProtocolListener` on provides `switch`.
    pub async fn bind_with<I>(switch: &Switch, protos: I) -> Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        switch.bind(protos).await
    }

    pub fn to_state(&self) -> ProtocolListenerState {
        self.0.clone()
    }

    /// Conver the switch into a [`Stream`](futures::Stream) object.
    pub fn into_incoming(
        self,
    ) -> impl futures::Stream<Item = Result<(ProtocolStream, String)>> + Unpin {
        Box::pin(futures::stream::unfold(self, |listener| async move {
            let res = listener.accept().await;
            Some((res, listener))
        }))
    }
}

impl Drop for ProtocolListener {
    fn drop(&mut self) {
        let state = self.to_state();
        spawn_ok(async move {
            state.close().await;
        });
    }
}

impl Deref for ProtocolListener {
    type Target = ProtocolListenerState;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
