use std::time::{Duration, Instant};

use clap::Parser;
use futures::{executor::block_on, AsyncReadExt, AsyncWriteExt};
use rand::{thread_rng, RngCore};
use rasi::timer::sleep;
use rasi_mio::{net::register_mio_network, timer::register_mio_timer};
use xstack::{multiaddr::Multiaddr, Switch, PROTOCOL_IPFS_PING};
use xstack_tcp::TcpTransport;

fn clap_parse_multiaddr(s: &str) -> Result<Vec<Multiaddr>, String> {
    let addrs = s
        .split(";")
        .map(|v| Multiaddr::try_from(v))
        .collect::<Result<Vec<Multiaddr>, xstack::multiaddr::Error>>()
        .map_err(|err| err.to_string())?;

    Ok(addrs)
}

type Multiaddrs = Vec<Multiaddr>;

#[derive(Parser, Debug)]
#[command(
    version,
    about,
    long_about = "This is a rp2p-based program to sniff the topology of a libp2p network"
)]
struct Client {
    /// The boostrap route table.
    #[arg(short, long, value_parser = clap_parse_multiaddr, default_value="/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ")]
    bootstrap: Multiaddrs,

    /// Use verbose output
    #[arg(short, long, default_value_t = false)]
    verbose: bool,
}

fn main() {
    register_mio_network();
    register_mio_timer();

    if let Err(err) = block_on(run_client()) {
        log::error!("Sniffier exit with error: {:#?}", err);
    }
}

async fn run_client() -> xstack::Result<()> {
    let config = Client::parse();

    let level = if config.verbose {
        log::LevelFilter::Trace
    } else {
        log::LevelFilter::Info
    };

    pretty_env_logger::formatted_timed_builder()
        .filter_level(level)
        .init();

    const VERSION: &str = env!("CARGO_PKG_VERSION");

    let switch = Switch::new(format!("rp2p-{}", VERSION))
        .transport(TcpTransport)
        .create()
        .await?;

    log::info!("Start switch: {}", switch.local_id());

    for raddr in config.bootstrap {
        log::info!("connect to peer: {}", raddr);

        let (mut stream, protocol_id) = switch.connect(raddr, [PROTOCOL_IPFS_PING]).await?;

        log::info!(
            "open stream, to={}, protocol={}",
            stream.public_key().to_peer_id(),
            protocol_id
        );

        loop {
            let mut buf = vec![0u8; 32];

            thread_rng().fill_bytes(&mut buf);

            let now = Instant::now();

            stream.write_all(&buf).await?;

            let mut echo = vec![0u8; 32];

            stream.read_exact(&mut echo).await?;

            assert_eq!(echo, buf);

            log::info!(
                "ping to={}, ttl={:?}",
                stream.public_key().to_peer_id(),
                now.elapsed()
            );

            sleep(Duration::from_secs(5)).await;
        }
    }

    Ok(())
}
