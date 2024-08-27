//! A plugin system for libp2p's application layer protocol.
//!

use std::{collections::HashMap, sync::Arc};

use futures::TryStreamExt;

use rasi::task::spawn_ok;

use crate::{driver_wrapper, Result, Switch, SwitchBuilder};

/// A libp2p protocol driver must implement the `Driver-*` traits in this module.
pub mod syscall {
    use std::io::Result;

    use async_trait::async_trait;

    use crate::{transport::Stream, Switch};

    use super::ProtocolHandler;

    /// A protocol server side code should implement this trait.
    #[async_trait]
    pub trait DriverProtocol: Sync + Send {
        /// Returns protocol display name.
        fn protos(&self) -> &'static [&'static str];

        async fn create(&self, switch: &Switch) -> Result<ProtocolHandler>;
    }

    /// A dyn object created by [`create`](DriverProtocol::create) function.
    #[async_trait]
    pub trait DriverProtocolHandler: Sync + Send {
        /// Handle a new incoming stream.
        async fn dispatch(&self, negotiated: &str, stream: Stream) -> Result<()>;
    }
}

driver_wrapper!(
["A type wrapper of [`DriverProtocol`](syscall::DriverProtocol)"]
Protocol[syscall::DriverProtocol]
);

driver_wrapper!(
["A type wrapper of [`DriverProtocolHandler`](syscall::DriverProtocolHandler)"]
ProtocolHandler[syscall::DriverProtocolHandler]
);

/// `ServeMux` is a protocol request mulitplexer.
pub struct ServeMux {
    protocols: Vec<Protocol>,
}

impl ServeMux {
    /// Create a new ServeMux instance.
    pub fn new() -> ServeMux {
        ServeMux {
            protocols: Default::default(),
        }
    }
    /// Add one `protocol` support.
    pub fn handle<P>(mut self, protocol: P) -> Self
    where
        P: syscall::DriverProtocol + 'static,
    {
        self.protocols.push(protocol.into());

        self
    }

    /// Starts the protocol stream dispatch loop until it exits the loop.
    pub async fn serve(self, switch: SwitchBuilder) -> Result<()> {
        let (runner, _) = self.create_switch(switch).await?;

        runner.dispatch().await?;

        Ok(())
    }

    async fn create_switch(self, mut switch: SwitchBuilder) -> Result<(ServeMuxRunner, Switch)> {
        for proto in &self.protocols {
            switch = switch.protos(proto.protos());
        }

        let switch = switch.create().await?;

        let mut handlers = HashMap::new();

        for proto in self.protocols {
            let handler = Arc::new(proto.create(&switch).await?);

            for id in proto.protos() {
                handlers.insert(id.to_string(), handler.clone());
            }
        }

        let runner = ServeMuxRunner {
            handlers: Arc::new(handlers),
            switch: switch.clone(),
        };

        Ok((runner, switch))
    }

    /// Create a background task to run protocol event dispatch loop.
    ///
    /// On success, returns created [`Switch`]
    pub async fn create(self, switch: SwitchBuilder) -> Result<Switch> {
        let (runer, switch) = self.create_switch(switch).await?;

        spawn_ok(async move {
            if let Err(err) = runer.dispatch().await {
                log::error!(target:"ServeMux", "stop serve: {}",err);
            } else {
                log::error!(target:"ServeMux", "stop serve");
            }
        });

        Ok(switch)
    }
}

#[derive(Clone)]
struct ServeMuxRunner {
    handlers: Arc<HashMap<String, Arc<ProtocolHandler>>>,
    switch: Switch,
}

impl ServeMuxRunner {
    async fn dispatch(self) -> Result<()> {
        let mut incoming = self.switch.into_incoming();

        while let Some((stream, negotiated)) = incoming.try_next().await? {
            let id = stream.id().to_string();

            log::info!(target:"ServerMux", "accept new stream, id={}, negotiated={}", id, negotiated);

            if let Some(handler) = self.handlers.get(&negotiated) {
                let handler = handler.clone();

                spawn_ok(async move {
                    if let Err(err) = handler.dispatch(&negotiated, stream).await {
                        log::info!(
                            target:"ServerMux",
                            "dispatch stream, id={}, negotiated={}, err={}",
                            id,
                            negotiated,
                            err
                        );
                    } else {
                        log::info!(target:"ServerMux", "dispatch stream ok, id={}, negotiated={}", id, negotiated,);
                    }
                })
            } else {
                log::warn!(
                    target:"ServerMux",
                    "can't dispatch stream, id={}, negotiated={},",
                    id,
                    negotiated
                );
            }
        }

        Ok(())
    }
}
