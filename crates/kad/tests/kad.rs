use std::{str::FromStr, sync::Once};

use rasi_mio::{net::register_mio_network, timer::register_mio_timer};
use xstack::{
    global_switch,
    identity::{Keypair, PeerId},
    PeerInfo, ProtocolStream, Switch,
};
use xstack_autonat::AutoNatClient;
use xstack_dnsaddr::DnsAddr;
use xstack_kad::{
    GetProviders, GetValue, KademliaRouter, KademliaRpc, PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD,
};
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
        .transport(TcpTransport)
        .transport(DnsAddr::new().await.unwrap())
        .create()
        .await
        .unwrap()
}

#[futures_test::test]
async fn find_node() {
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

    loop {
        let peer_id = PeerId::random();

        let peer_info = kad.find_node(&peer_id).await.unwrap();

        log::info!("find_node: {}, {:?}", peer_id, peer_info);

        log::info!("kad({}), autonat({:?})", kad.len(), switch.nat().await);
    }
}

#[futures_test::test]
async fn find_node_1() {
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

    let peer_id = PeerId::from_str("12D3KooWLjoYKVxbGGwLwaD4WHWM9YiDpruCYAoFBywJu3CJppyB").unwrap();

    let peer_info = kad.find_node(&peer_id).await.unwrap();

    log::info!("find_node: {}, {:?}", peer_id, peer_info);
}

#[futures_test::test]
async fn put_value() {
    let switch = init().await;

    let (stream, _) = ProtocolStream::connect_with(
        &switch,
        "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
        [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
    )
    .await
    .unwrap();

    let keypair = Keypair::generate_ed25519();

    let id = PeerId::from_public_key(&keypair.public());
    let value = keypair.public().encode_protobuf();

    let mut key = "/pk/".as_bytes().to_vec();

    key.append(&mut id.to_bytes());

    stream
        .kad_put_value(&key, &value, 1024 * 1024)
        .await
        .unwrap();

    let (stream, _) = ProtocolStream::connect_with(
        &switch,
        "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
        [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
    )
    .await
    .unwrap();

    let GetValue {
        closer_peers: _,
        value: get_value,
    } = stream.kad_get_value(key, 1024 * 1024).await.unwrap();

    assert_eq!(get_value, Some(value));
}

#[futures_test::test]
async fn add_provider() {
    let switch = init().await;

    let (stream, _) = ProtocolStream::connect_with(
        &switch,
        "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
        [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
    )
    .await
    .unwrap();

    let id = PeerId::random();

    let peer_info = PeerInfo {
        id: global_switch().local_id().clone(),
        addrs: vec!["/ip4/89.58.16.110/udp/37530/quic-v1".parse().unwrap()],
        ..Default::default()
    };

    stream
        .kad_add_provider(id.to_bytes(), &peer_info)
        .await
        .unwrap();

    let (stream, _) = ProtocolStream::connect_with(
        &switch,
        "QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
        [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
    )
    .await
    .unwrap();

    let GetProviders {
        closer_peers: _,
        provider_peers,
    } = stream
        .kad_get_providers(id.to_bytes(), 1024 * 1024)
        .await
        .unwrap();

    assert_eq!(provider_peers, vec![peer_info]);
}

#[futures_test::test]
async fn get_provider() {
    let switch = init().await;

    let (stream, _) = ProtocolStream::connect_with(
        &switch,
        "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
        [PROTOCOL_IPFS_KAD, PROTOCOL_IPFS_LAN_KAD],
    )
    .await
    .unwrap();

    let cid = bs58::decode("QmdmQXB2mzChmMeKY47C43LxUdg1NDJ5MWcKMKxDu7RgQm")
        .into_vec()
        .unwrap();

    let GetProviders {
        closer_peers,
        provider_peers,
    } = stream.kad_get_providers(cid, 1024 * 1024).await.unwrap();

    log::trace!("{:?}", closer_peers);
    log::trace!("{:?}", provider_peers);
}

#[futures_test::test]
async fn test_ping() {
    let switch = init().await;

    ProtocolStream::ping_with(
        &switch,
        "/ip4/107.173.86.71/udp/4001/quic/p2p/12D3KooWGDrZPTx1LrGevpVj1Djn9dni9cDJRYSe9MtMLHmwJQNz",
    )
    .await
    .unwrap();
}
