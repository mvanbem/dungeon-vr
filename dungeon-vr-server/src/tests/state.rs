use std::marker::PhantomData;
use std::time::Duration;

use dungeon_vr_cryptography::SharedSecret;
use dungeon_vr_shared::protocol::packet::Packet;
use tokio::time::{interval, sleep, timeout_at, Instant};

use crate::testing::{
    init_with_connected_connection, init_with_pending_connection, make_network_and_inner_server,
    recv_packet, run_server_and_test_with_timeout, run_test_with_timeout, FakeAddr,
    InitWithConnectedConnection, InitWithPendingConnection,
};
use crate::{
    Connection, ConnectionVariant, DisconnectingConnection, CLIENT_TIMEOUT_INTERVAL,
    DISCONNECT_INTERVAL, DISCONNECT_PACKET_COUNT,
};

#[tokio::test(start_paused = true)]
async fn pending_connection_should_send_challenges() {
    let InitWithPendingConnection {
        network,
        server,
        client_private_key,
        server_public_key,
        token,
        ..
    } = init_with_pending_connection();
    let socket = network.bind(FakeAddr::Client1);

    run_server_and_test_with_timeout(server, async move {
        for _ in 0..3 {
            let packet = match recv_packet(&socket).await {
                Packet::ConnectChallenge(packet) => packet,
                _ => unreachable!(),
            };
            assert_eq!(server_public_key, packet.server_public_key);
            assert_eq!(
                token,
                packet
                    .sealed_payload
                    .open(
                        &client_private_key
                            .exchange(&packet.server_public_key)
                            .unwrap(),
                    )
                    .unwrap(),
            );
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_should_time_out() {
    let InitWithPendingConnection { mut server, .. } = init_with_pending_connection();

    run_test_with_timeout(async move {
        loop {
            server.run_once_for_test().await;

            let connection = server.connections.get(&FakeAddr::Client1).unwrap();
            if matches!(connection.variant, ConnectionVariant::Disconnecting(_)) {
                // Success.
                break;
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_should_send_keepalives() {
    let InitWithConnectedConnection {
        network,
        mut server,
        shared_secret,
    } = init_with_connected_connection();
    server.tick_interval = interval(Duration::from_secs(60));
    let socket = network.bind(FakeAddr::Client1);

    run_server_and_test_with_timeout(server, async move {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let packet = match timeout_at(deadline, recv_packet(&socket)).await.unwrap() {
                Packet::Keepalive(packet) => packet,
                _ => unreachable!(),
            };
            packet.open(&shared_secret).unwrap();
            // Success.
            break;
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_should_time_out() {
    let InitWithConnectedConnection { mut server, .. } = init_with_connected_connection();

    run_test_with_timeout(async move {
        loop {
            server.run_once_for_test().await;

            let connection = server.connections.get(&FakeAddr::Client1).unwrap();
            if matches!(connection.variant, ConnectionVariant::Disconnecting(_)) {
                // Success.
                break;
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn shuttingdown_connection_should_send_disconnects() {
    let (network, mut server) = make_network_and_inner_server();
    let shared_secret = SharedSecret::gen();
    server.connections.insert(
        FakeAddr::Client1,
        Connection {
            shared_secret,
            timeout: Some(Box::pin(sleep(CLIENT_TIMEOUT_INTERVAL))),
            variant: ConnectionVariant::Disconnecting(DisconnectingConnection {
                interval: interval(DISCONNECT_INTERVAL),
                packets_to_send: DISCONNECT_PACKET_COUNT,
            }),
            _phantom_socket: PhantomData,
        },
    );
    let socket = network.bind(FakeAddr::Client1);

    run_server_and_test_with_timeout(server, async move {
        for _ in 0..DISCONNECT_PACKET_COUNT {
            let packet = match recv_packet(&socket).await {
                Packet::Disconnect(packet) => packet,
                _ => unreachable!(),
            };
            packet.open(&shared_secret).unwrap();
        }
    })
    .await;
}
