use dungeon_vr_cryptography::{PrivateKey, SharedSecret};
use dungeon_vr_shared::protocol::challenge_token::ChallengeToken;
use dungeon_vr_shared::protocol::connect_init_packet::ConnectInitPacket;
use dungeon_vr_shared::protocol::packet::Packet;
use dungeon_vr_shared::protocol::sealed::Sealed;
use dungeon_vr_shared::protocol::GAME_ID;

use crate::testing::{
    init_with_connected_connection, init_with_pending_connection, make_network_and_inner_server,
    run_test_with_timeout, send_bytes_to, send_packet_to, FakeAddr, InitWithConnectedConnection,
    InitWithPendingConnection,
};
use crate::ConnectionVariant;

#[tokio::test(start_paused = true)]
async fn no_connection_recv_empty_should_ignore() {
    run_test_with_timeout(async move {
        let (network, mut server) = make_network_and_inner_server();
        let socket = network.bind(FakeAddr::Client1);
        send_bytes_to(&socket, b"", FakeAddr::Server).await;

        server.run_once_for_test().await;

        assert!(server.connections.get(&FakeAddr::Client1).is_none());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn no_connection_recv_malformed_should_ignore() {
    run_test_with_timeout(async move {
        let (network, mut server) = make_network_and_inner_server();
        let socket = network.bind(FakeAddr::Client1);
        send_bytes_to(&socket, b"\x01ConnectInit but too short", FakeAddr::Server).await;

        server.run_once_for_test().await;

        assert!(server.connections.get(&FakeAddr::Client1).is_none());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn no_connection_recv_connectinit_should_create_pending_connection() {
    run_test_with_timeout(async move {
        let (network, mut server) = make_network_and_inner_server();
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::ConnectInit(ConnectInitPacket {
                game_id: GAME_ID,
                client_public_key: PrivateKey::gen().to_public(),
            }),
            FakeAddr::Server,
        )
        .await;

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        assert!(matches!(connection.variant, ConnectionVariant::Pending(_)));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_connectinit_should_ignore() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            mut server,
            server_public_key,
            token,
            ..
        } = init_with_pending_connection();
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::ConnectInit(ConnectInitPacket {
                game_id: GAME_ID,
                client_public_key: PrivateKey::gen().to_public(),
            }),
            FakeAddr::Server,
        )
        .await;

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        let pending = match &connection.variant {
            ConnectionVariant::Pending(pending) => pending,
            _ => unreachable!(),
        };
        assert_eq!(server_public_key, pending.server_public_key);
        assert_eq!(token, pending.token);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_connectresponse_should_connect() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            mut server,
            shared_secret,
            token,
            ..
        } = init_with_pending_connection();
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::ConnectResponse(Sealed::seal(token, &shared_secret)),
            FakeAddr::Server,
        )
        .await;

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        assert!(matches!(
            connection.variant,
            ConnectionVariant::Connected(_),
        ));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_bad_signature_connectresponse_should_discard() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            mut server,
            token,
            ..
        } = init_with_pending_connection();
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::ConnectResponse(Sealed::seal(token, &SharedSecret::gen())),
            FakeAddr::Server,
        )
        .await;

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        assert!(matches!(connection.variant, ConnectionVariant::Pending(_)));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_bad_token_connectresponse_should_discard() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            mut server,
            shared_secret,
            ..
        } = init_with_pending_connection();
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::ConnectResponse(Sealed::seal(ChallengeToken::gen(), &shared_secret)),
            FakeAddr::Server,
        )
        .await;

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        assert!(matches!(connection.variant, ConnectionVariant::Pending(_)));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_recv_keepalive_should_refresh_timeout() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            mut server,
            shared_secret,
        } = init_with_connected_connection();
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::Keepalive(Sealed::seal((), &shared_secret)),
            FakeAddr::Server,
        )
        .await;
        let initial_deadline = server
            .connections
            .get(&FakeAddr::Client1)
            .unwrap()
            .timeout
            .as_ref()
            .unwrap()
            .deadline();

        server.run_once_for_test().await;

        let final_deadline = server
            .connections
            .get(&FakeAddr::Client1)
            .unwrap()
            .timeout
            .as_ref()
            .unwrap()
            .deadline();
        assert!(final_deadline > initial_deadline);
    })
    .await;
}
