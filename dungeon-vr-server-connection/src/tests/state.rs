use std::num::NonZeroU8;

use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_shared::net_game::PlayerId;

use crate::testing::{
    init_with_connected_connection, init_with_disconnecting_connection,
    init_with_pending_connection, recv_packet, run_test_with_timeout, FakeAddr,
    InitWithConnectedConnection, InitWithDisconnectingConnection, InitWithPendingConnection,
};
use crate::{Event, DISCONNECT_PACKET_COUNT};

#[tokio::test(start_paused = true)]
async fn pending_connection_should_send_challenges() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            cancel_guard: _cancel_guard,
            client_private_key,
            server_public_key,
            token,
            ..
        } = init_with_pending_connection();

        let socket = network.bind(FakeAddr::Client1);
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
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_pending_connection();

        assert_eq!(Event::PeerDisconnected, events.recv().await.unwrap());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_should_send_keepalives() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            shared_secret,
            ..
        } = init_with_connected_connection();

        let socket = network.bind(FakeAddr::Client1);
        for _ in 0..3 {
            let packet = match recv_packet(&socket).await {
                Packet::Keepalive(packet) => packet,
                _ => unreachable!(),
            };
            let () = packet.open(&shared_secret).unwrap();
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_should_time_out() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_connected_connection();

        assert_eq!(
            Event::PlayerDisconnected {
                player_id: PlayerId(NonZeroU8::new(1).unwrap())
            },
            events.recv().await.unwrap(),
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn disconnecting_connection_should_send_disconnects() {
    run_test_with_timeout(async move {
        let InitWithDisconnectingConnection {
            network,
            cancel_guard: _cancel_guard,
            shared_secret,
            ..
        } = init_with_disconnecting_connection();

        let socket = network.bind(FakeAddr::Client1);
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
