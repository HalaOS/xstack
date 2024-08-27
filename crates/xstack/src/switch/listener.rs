use rasi::task::spawn_ok;

use crate::{transport::ProtocolStream, Result};

use super::{ListenerId, Switch, SwitchEvent};

/// A server-side socket that accept new inbound [`ProtocolStream`]
pub struct ProtocolListener {
    id: ListenerId,
    switch: Switch,
}

impl ProtocolListener {
    pub(super) fn new(id: ListenerId, switch: Switch) -> Self {
        Self { id, switch }
    }

    /// Create a new `ProtocolListener` with `protos`.
    ///
    /// This function internally calls [`global_switch`] to get [`Switch`] instance,
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
}

impl Drop for ProtocolListener {
    fn drop(&mut self) {
        let switch = self.switch.clone();
        let id = self.id;
        spawn_ok(async move {
            switch.mutable.lock().await.close_protocol_listener(&id);
        })
    }
}

impl ProtocolListener {
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
                .wait(&SwitchEvent::Accept(self.id), ());
        }
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
