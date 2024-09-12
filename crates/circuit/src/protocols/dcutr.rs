use futures::TryStreamExt;
use rasi::task::spawn_ok;
use xstack::{
    multiaddr::{is_quic_transport, is_tcp_transport},
    ProtocolStream, Switch,
};

use crate::{DCUtRRpc, Result, PROTOCOL_DCUTR};

/// A `DCUtR` protocol server-side implementation.
#[derive(Clone)]
pub struct DCUtRUpgrader {
    switch: Switch,
}

impl DCUtRUpgrader {
    /// Bind a new *autonat* client instance to global context `Switch`.
    #[cfg(feature = "global_register")]
    #[cfg_attr(docsrs, doc(cfg(feature = "global_register")))]
    pub fn bind() {
        use xstack::global_switch;

        Self::bind_with(global_switch())
    }

    /// Bind a new *autonat* client instance to `Switch`
    pub fn bind_with(switch: &Switch) {
        let client = Self {
            switch: switch.clone(),
        };

        spawn_ok(client.server_loop());
    }

    async fn server_loop(self) {
        if let Err(err) = self.server_loop_prv().await {
            log::error!("DCUtR upgrader stopped with error: {}", err);
        }
    }

    async fn server_loop_prv(self) -> Result<()> {
        let mut incoming = self.switch.bind([PROTOCOL_DCUTR]).await?.into_incoming();

        while let Some((stream, _)) = incoming.try_next().await? {
            let this = self.clone();

            spawn_ok(async move {
                let peer_id = stream.public_key().to_peer_id();

                log::trace!("DCUtR upgrade from={}", peer_id);

                if let Err(err) = this.upgrade(stream).await {
                    log::error!("DCUtR upgrade from={}\r\terror={}", peer_id, err);
                }
            });
        }

        Ok(())
    }

    async fn upgrade(self, mut stream: ProtocolStream) -> Result<()> {
        let raddrs = DCUtRRpc::dcutr_recv_connect(&mut stream, self.switch.max_packet_size).await?;

        log::trace!("Connect: {:?}", raddrs);

        let observed_addrs = self.switch.observed_addrs().await;

        let mut laddrs = vec![];
        let mut sync_addrs = vec![];

        for addr in raddrs {
            if is_quic_transport(&addr) {
                if let Some(laddr) = observed_addrs.iter().find(|raddr| is_quic_transport(raddr)) {
                    laddrs.push(laddr.clone());
                    sync_addrs.push(addr);
                    continue;
                }
            }

            if is_tcp_transport(&addr) {
                if let Some(laddr) = observed_addrs.iter().find(|raddr| is_tcp_transport(raddr)) {
                    laddrs.push(laddr.clone());
                    sync_addrs.push(addr);
                    continue;
                }
            }
        }

        log::trace!("Connect response: {:?}", laddrs);

        if sync_addrs.is_empty() {
            log::error!("Unable to find any suitable multaddr for hole punching.");
            return Ok(());
        }

        DCUtRRpc::dcutr_send_connect(&mut stream, &laddrs).await?;

        DCUtRRpc::dcutr_recv_sync(&mut stream, self.switch.max_packet_size).await?;

        log::info!("Sync, start hole punching.");

        let mut last_error = None;

        for addr in sync_addrs {
            match self
                .switch
                .transport_connect_and_replace(&addr, Some(stream.conn_id()))
                .await
            {
                Ok(_) => {
                    log::trace!("hole punching to {}, is success.", addr);
                    return Ok(());
                }
                Err(err) => {
                    log::error!("try hole punching to {} with error: {}", addr, err);
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap().into())
    }
}
