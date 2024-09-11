use std::{
    io::{Error, ErrorKind, Result},
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};

use async_trait::async_trait;

use chrono::Local;
use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    SinkExt, StreamExt, TryStreamExt,
};
use futures_map::FuturesUnorderedMap;
use rasi::{task::spawn_ok, timer::sleep};
use xstack::{
    events,
    multiaddr::{Multiaddr, Protocol},
    transport_syscall::DriverListener,
    AutoNAT, EventSource, P2pConn, ProtocolListener, ProtocolStream, Switch, XStackRpc,
    PROTOCOL_IPFS_PING,
};
use xstack_tls::{create_ssl_acceptor, TlsConn};

use crate::{CircuitV2Rpc, PROTOCOL_CIRCUIT_RELAY_HOP, PROTOCOL_CIRCUIT_RELAY_STOP};

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

/// A builder for [`CircuitStopServer`]
#[derive(Clone)]
pub struct CircuitStopServerBuilder {
    reservations: Arc<AtomicUsize>,
    activities: Arc<AtomicUsize>,
    incoming_buffer: usize,
    channel_limits: usize,
    ping_duration: Duration,
    switch: Switch,
}

impl CircuitStopServerBuilder {
    /// Override default `incoming_buffer` configuration, the default value is `100`.
    ///
    /// This value limits the maximum length of incoming stream queue.
    pub fn incoming_buffer(mut self, value: usize) -> Self {
        self.incoming_buffer = value;
        self
    }

    /// Override default `channel_limits` configuration, the default value is `5`.
    ///
    /// This value limits the maximun number of [`reservation`] channels.
    ///
    ///
    /// [`reservation`]: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md#reservation
    pub fn channel_limits(mut self, value: NonZeroUsize) -> Self {
        self.channel_limits = value.into();
        self
    }

    /// Consume `CircuitStopServer` configuration and start the server.
    pub fn start(self) -> CircuitStopServer {
        spawn_ok(self.clone().run_stop_accept());

        spawn_ok(self.clone().run_hop_client());

        CircuitStopServer {
            reservations: self.reservations,
        }
    }

    async fn run_hop_client(self) {
        let mut event_source = EventSource::<events::Network>::bind_with(
            &self.switch,
            NonZeroUsize::new(100).unwrap(),
        )
        .await;

        loop {
            // check autonat status.
            loop {
                let nat = self.switch.nat().await;
                log::trace!("hop client check network: {:?}", nat);

                if nat == AutoNAT::NAT {
                    break;
                }

                if event_source.next().await.is_none() {
                    log::trace!("switch closed.");
                    return;
                };
            }

            log::trace!("start circuit reservation client.");

            self.run_reservation_client().await;
        }
    }
    async fn run_stop_accept(self) {
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

        let listener = match self.switch.bind([PROTOCOL_CIRCUIT_RELAY_STOP]).await {
            Ok(listener) => listener,
            Err(err) => {
                log::error!(
                    "Start protocol listener '{}' with error: {}",
                    PROTOCOL_CIRCUIT_RELAY_STOP,
                    err
                );
                return;
            }
        };

        self.protocol_incoming_loop(
            sender,
            listener,
            self.activities.clone(),
            self.switch.max_packet_size,
        )
        .await;
    }

    async fn run_reservation_client(&self) {
        let mut unordered = FuturesUnorderedMap::new();

        let mut id = 0;

        for _ in 0..self.channel_limits {
            unordered.insert(id, self.clone().reservation_client_loop());
            id += 1;
        }

        while let Some(_) = unordered.next().await {
            if self.switch.nat().await == AutoNAT::NAT {
                while unordered.len() < self.channel_limits {
                    unordered.insert(id, self.clone().reservation_client_loop());
                    id += 1;
                }
            }
        }
    }

    async fn reservation_client_loop(self) {
        if let Err(err) = self.reservation_client_loop_prv().await {
            log::error!("reservation_client_loop, stopped with error: {}", err);
        }
    }

    async fn reservation_client_loop_prv(self) -> Result<()> {
        let peers = self
            .switch
            .choose_peers(PROTOCOL_CIRCUIT_RELAY_HOP, 1)
            .await?;

        if peers.is_empty() {
            log::trace!("hop client, sleep...");
            // retry after 10s.
            sleep(Duration::from_secs(10)).await;
            return Ok(());
        }

        let peer_id = peers[0];

        let (mut stream, _) = self
            .switch
            .connect(&peer_id, [PROTOCOL_CIRCUIT_RELAY_HOP])
            .await?;

        let reservation =
            CircuitV2Rpc::circuit_v2_hop_reserve(&mut stream, self.switch.max_packet_size).await?;

        let peer_addr = stream.peer_addr().clone();

        log::trace!(
            "reserve from {}, expire={}, {}",
            peer_addr,
            chrono::DateTime::<Local>::from(reservation.expire).to_rfc3339(),
            reservation
                .limit
                .map(|limit| limit.to_string())
                .unwrap_or("".to_owned())
        );

        self.reservations.fetch_add(1, Ordering::Relaxed);

        let mut stream = match self.switch.connect(&peer_id, [PROTOCOL_IPFS_PING]).await {
            Ok((stream, _)) => stream,
            Err(err) => {
                log::trace!("reservation from {}, ping timeout/open failed", peer_addr);
                self.reservations.fetch_sub(1, Ordering::Relaxed);
                return Err(err.into());
            }
        };

        let nat_addrs = reservation
            .addrs
            .iter()
            .map(|addr| addr.clone().with(Protocol::P2pCircuit))
            .collect::<Vec<_>>();

        self.switch.set_nat_addrs(nat_addrs.clone()).await;

        while SystemTime::now() < reservation.expire {
            let now = Instant::now();

            if let Err(err) = XStackRpc::xstack_ping(&mut stream).await {
                self.reservations.fetch_sub(1, Ordering::Relaxed);
                self.switch.remove_nat_addrs(&nat_addrs).await;
                log::trace!("reservation from {}, ping timeout/open failed", peer_addr);
                return Err(err.into());
            }

            log::trace!(
                "reservation from {}, ping succ, time={:?}",
                peer_addr,
                now.elapsed()
            );

            // break the ping loop, when switch network was changed.
            if self.switch.nat().await != AutoNAT::NAT {
                self.reservations.fetch_sub(1, Ordering::Relaxed);
                return Ok(());
            }

            sleep(self.ping_duration).await;
        }

        // reservation timeout approaching.
        self.switch.remove_nat_addrs(&nat_addrs).await;

        log::trace!("reservation from {}, timeout", peer_addr);

        Ok(())
    }

    async fn protocol_incoming_loop(
        &self,
        sender: Sender<P2pConn>,
        listener: ProtocolListener,
        activities: Arc<AtomicUsize>,
        max_packet_size: usize,
    ) {
        if let Err(err) =
            Self::circuit_stop_server_loop_prv(self, sender, listener, activities, max_packet_size)
                .await
        {
            log::error!("circuit_stop_server_loop, stopped with error {}", err)
        } else {
            log::info!("circuit_stop_server_loop, stopped.");
        }
    }

    async fn circuit_stop_server_loop_prv(
        &self,
        sender: Sender<P2pConn>,
        listener: ProtocolListener,
        activities: Arc<AtomicUsize>,
        max_packet_size: usize,
    ) -> Result<()> {
        let mut incoming = listener.into_incoming();

        while let Some((stream, _)) = incoming.try_next().await? {
            log::trace!(
                "[{}] new incoming stream from {}",
                PROTOCOL_CIRCUIT_RELAY_STOP,
                stream.peer_addr()
            );

            let sender = sender.clone();
            let activities = activities.clone();

            let this = self.clone();

            spawn_ok(async move {
                let peer_id = stream.public_key().to_peer_id();
                if let Err(err) = this
                    .handle_stop_incoming(stream, sender, activities, max_packet_size)
                    .await
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
        self,
        mut stream: ProtocolStream,
        mut sender: Sender<P2pConn>,
        activities: Arc<AtomicUsize>,
        max_packet_size: usize,
    ) -> Result<()> {
        CircuitV2Rpc::circuit_v2_stop_connect_accept(&mut stream, max_packet_size).await?;

        let local_addr = stream.local_addr().clone();
        let peer_addr = stream.peer_addr().clone();

        let ssl_acceptor = create_ssl_acceptor(&self.switch).await?;

        let conn =
            TlsConn::accept(stream, local_addr, peer_addr, &ssl_acceptor, activities).await?;

        Ok(sender
            .send(conn.into())
            .await
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, ""))?)
    }
}

/// A [`stop`] protocol server side implementation.
///
/// [`stop`]: https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md#stop-protocol
pub struct CircuitStopServer {
    reservations: Arc<AtomicUsize>,
}

impl CircuitStopServer {
    /// Bind `CircuitStopServer` to the global context `Switch`.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub fn new() -> CircuitStopServerBuilder {
        use xstack::global_switch;

        Self::bind_with(global_switch())
    }

    /// Bind `CircuitStopServer` to a `Switch`.
    pub fn bind_with(switch: &Switch) -> CircuitStopServerBuilder {
        CircuitStopServerBuilder {
            reservations: Default::default(),
            switch: switch.clone(),
            incoming_buffer: 100,
            activities: Default::default(),
            channel_limits: 2,
            ping_duration: Duration::from_secs(5),
        }
    }

    /// Returns the count of valid reservations.
    pub fn reservations(&self) -> usize {
        self.reservations.load(Ordering::Relaxed)
    }
}
