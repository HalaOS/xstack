use std::{
    sync::Once,
    time::{Duration, Instant},
};

use rasi::timer::sleep;
use rasi_mio::{net::register_mio_network, timer::register_mio_timer};

use xstack::{
    identity::PeerId,
    multiaddr::{Multiaddr, Protocol},
    Switch, XStackRpc, PROTOCOL_IPFS_PING,
};
use xstack_autonat::AutoNatClient;
use xstack_circuit::{CircuitStopServer, CircuitTransport};
use xstack_dnsaddr::DnsAddr;
use xstack_kad::KademliaRouter;
use xstack_quic::QuicTransport;
use xstack_tcp::TcpTransport;

async fn init() -> Switch {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        _ = pretty_env_logger::try_init_timed();

        register_mio_network();
        register_mio_timer();
    });

    Switch::new("kad-test")
        .transport(QuicTransport::default())
        .transport(TcpTransport::default())
        .transport(DnsAddr::new().await.unwrap())
        .transport(CircuitTransport::default())
        .create()
        .await
        .unwrap()
}

#[futures_test::test]
async fn client_connect() {
    let switch = init().await;

    let kad = KademliaRouter::with(&switch)
            .with_seeds([
                "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
		        "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
		        "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
		        "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
		        "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
		        "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
            ])
            .await
            .unwrap();

    AutoNatClient::bind_with(&switch);

    let peer_id = "12D3KooWLjoYKVxbGGwLwaD4WHWM9YiDpruCYAoFBywJu3CJppyB"
        .parse()
        .unwrap();

    let now = Instant::now();

    let peer_info = kad.find_node(&peer_id).await.unwrap().expect("found peer");

    log::trace!(
        "kad search peer_d={}, times={:?} success",
        peer_id,
        now.elapsed(),
    );

    log::trace!("add circuit addresses={:#?}", peer_info.addrs);

    let circuit_suffix = Multiaddr::empty().with(Protocol::P2pCircuit);

    let addrs = peer_info
        .addrs
        .iter()
        .flat_map(|addr| {
            if addr.ends_with(&circuit_suffix) {
                Some(addr.clone().with_p2p(peer_id))
            } else {
                None
            }
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    log::trace!("add circuit addresses={:#?}", addrs);

    let now = Instant::now();

    let (stream, _) = switch.connect(addrs, [PROTOCOL_IPFS_PING]).await.unwrap();

    log::trace!(
        "circuit_v2 connect peer_id={}: times={:?}, raddr={}",
        peer_id,
        now.elapsed(),
        stream.peer_addr(),
    );

    stream.xstack_ping().await.unwrap();

    log::trace!(
        "circuit_v2 ping peer_id={}: times={:?}",
        peer_id,
        now.elapsed()
    );
}

#[futures_test::test]
async fn stop_server() {
    let switch = init().await;

    let kad = KademliaRouter::with(&switch)
            .with_seeds([
                "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
		        "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
		        "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
		        "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
		        "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
		        "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
            ])
            .await
            .unwrap();

    AutoNatClient::bind_with(&switch);

    let peer_id = PeerId::random();

    let now = Instant::now();

    let _ = kad.find_node(&peer_id).await.unwrap();

    log::trace!("kad search peer_d={}, times={:?}", peer_id, now.elapsed(),);

    let stop_server = CircuitStopServer::bind_with(&switch).start();

    while stop_server.reservations() == 0 {
        log::trace!("waiting reserve calls... {}", stop_server.reservations());
        sleep(Duration::from_secs(4)).await;
    }

    sleep(Duration::from_secs(4)).await;

    let circuit_suffix = Multiaddr::empty().with(Protocol::P2pCircuit);

    let addrs = switch
        .listen_addrs()
        .await
        .iter()
        .flat_map(|addr| {
            if addr.ends_with(&circuit_suffix) {
                Some(addr.clone().with_p2p(switch.local_id().clone()))
            } else {
                None
            }
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    log::info!("listen on: {:#?}", addrs);

    let switch_2 = init().await;

    let (stream, _) = switch_2
        .connect(addrs.as_slice(), [PROTOCOL_IPFS_PING])
        .await
        .unwrap();

    stream.xstack_ping().await.unwrap();
}
