use std::{
    io::{Error, ErrorKind, Result},
    num::NonZeroUsize,
    sync::{atomic::AtomicUsize, Arc},
};

use async_trait::async_trait;

use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    SinkExt, StreamExt, TryStreamExt,
};
use rasi::task::spawn_ok;
use xstack::{
    events,
    multiaddr::{Multiaddr, Protocol},
    transport_syscall::DriverListener,
    AutoNAT, EventSource, P2pConn, ProtocolListener, ProtocolStream, Switch,
};
use xstack_tls::{create_ssl_acceptor, SslAcceptor, TlsConn};

use crate::{CircuitV2Rpc, PROTOCOL_CIRCUIT_RELAY_HOP};

struct CircuitStopListener(Receiver<P2pConn>);

#[async_trait]
impl DriverListener for CircuitStopListener {
    /// Accept next incoming connection between local and peer.
    async fn accept(&mut self) -> Result<P2pConn> {
        self.0.next().await.ok_or(Error::new(
            ErrorKind::BrokenPipe,
            "CircuitStopListener broken.",
        ))
    }

    /// Returns the local address that this listener is bound to.
    fn local_addr(&self) -> Result<Multiaddr> {
        Ok(Multiaddr::empty().with(Protocol::P2pCircuit))
    }
}

/// A [`stop`] protocol server side implementation.
///
/// [`stop`]: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md#stop-protocol
pub struct CircuitStopServer {
    activities: Arc<AtomicUsize>,
    incoming_buffer: usize,
    switch: Switch,
}

impl CircuitStopServer {
    /// Bind `CircuitStopServer` to a `Switch`.
    ///
    /// # Parameters
    ///
    /// - `incoming_buffer` the incoming connecton queue maximun buffer size.
    pub fn bind_with(switch: &Switch, incoming_buffer: usize) {
        let server = CircuitStopServer {
            switch: switch.clone(),
            incoming_buffer,
            activities: Default::default(),
        };

        spawn_ok(server.listen_on_nat_changed());
    }

    async fn listen_on_nat_changed(self) {
        let mut event_source = EventSource::<events::Network>::bind_with(
            &self.switch,
            NonZeroUsize::new(100).unwrap(),
        )
        .await;

        loop {
            if self.switch.nat().await != AutoNAT::Unknown {
                break;
            }

            if event_source.next().await.is_none() {
                log::trace!("switch closed.");
                return;
            };
        }

        if self.switch.nat().await == AutoNAT::NAT {
            self.run_server().await;
        }
    }

    async fn run_server(self) {
        let ssl_acceptor = match create_ssl_acceptor(&self.switch).await {
            Ok(ssl_acceptor) => ssl_acceptor,
            Err(err) => {
                log::error!("create 'ssl_acceptor' with error, {}", err);
                return;
            }
        };

        let (sender, receiver) = channel(self.incoming_buffer);

        if let Err(err) = self
            .switch
            .transport_bind_with(CircuitStopListener(receiver).into())
            .await
        {
            log::error!("bind 'CircuitStopListener' with error, {}", err);
            return;
        }

        // start `/libp2p/circuit/relay/0.2.0/stop` listener.

        let listener = match self.switch.bind([PROTOCOL_CIRCUIT_RELAY_HOP]).await {
            Ok(listener) => listener,
            Err(err) => {
                log::error!(
                    "Start protocol listener '{}' with error: {}",
                    PROTOCOL_CIRCUIT_RELAY_HOP,
                    err
                );
                return;
            }
        };

        spawn_ok(Self::protocol_incoming_loop(
            ssl_acceptor,
            sender,
            listener,
            self.activities.clone(),
        ));

        if let Err(err) = self.run_reservation_client().await {
            log::error!("hop reservation client stopped, {}", err);
        }
    }

    async fn run_reservation_client(self) -> Result<()> {
        Ok(())
    }

    async fn protocol_incoming_loop(
        ssl_acceptor: SslAcceptor,
        sender: Sender<P2pConn>,
        listener: ProtocolListener,
        activities: Arc<AtomicUsize>,
    ) {
        if let Err(err) =
            Self::circuit_stop_server_loop_prv(ssl_acceptor, sender, listener, activities).await
        {
            log::error!("circuit_stop_server_loop, stopped with error {}", err)
        } else {
            log::info!("circuit_stop_server_loop, stopped.");
        }
    }

    async fn circuit_stop_server_loop_prv(
        ssl_acceptor: SslAcceptor,
        sender: Sender<P2pConn>,
        listener: ProtocolListener,
        activities: Arc<AtomicUsize>,
    ) -> Result<()> {
        let mut incoming = listener.into_incoming();

        while let Some((stream, _)) = incoming.try_next().await? {
            let ssl_acceptor = ssl_acceptor.clone();
            let sender = sender.clone();
            let activities = activities.clone();
            spawn_ok(async move {
                let peer_id = stream.public_key().to_peer_id();
                if let Err(err) =
                    Self::handle_stop_incoming(ssl_acceptor, stream, sender, activities).await
                {
                    log::error!(
                        "handle incoming({}) stop stream with error: {}",
                        peer_id,
                        err
                    );
                }
            });
        }
        Ok(())
    }

    async fn handle_stop_incoming(
        ssl_acceptor: SslAcceptor,
        mut stream: ProtocolStream,
        mut sender: Sender<P2pConn>,
        activities: Arc<AtomicUsize>,
    ) -> Result<()> {
        CircuitV2Rpc::circuit_v2_stop_connect_accept(&mut stream, 1024).await?;

        let local_addr = stream.local_addr().clone();
        let peer_addr = stream.peer_addr().clone();

        let conn =
            TlsConn::accept(stream, local_addr, peer_addr, &ssl_acceptor, activities).await?;

        Ok(sender
            .send(conn.into())
            .await
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, ""))?)
    }
}
