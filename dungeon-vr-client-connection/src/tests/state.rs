use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_protocol::GAME_ID;

use crate::testing::{
    init_with_connected_connection, init_with_connecting_connection,
    init_with_responding_connection, recv_packet, run_test_with_timeout, FakeAddr,
    InitWithConnectedConnection, InitWithConnectingConnection, InitWithRespondingConnection,
};
use crate::Event;

#[tokio::test(start_paused = true)]
async fn connecting_should_send_challenges() {
    run_test_with_timeout(async move {
        let InitWithConnectingConnection {
            network,
            cancel_guard: _cancel_guard,
            client_public_key,
            ..
        } = init_with_connecting_connection();

        let socket = network.connect(FakeAddr::Server, FakeAddr::Client);
        for _ in 0..3 {
            let packet = match recv_packet(&socket).await {
                Packet::ConnectInit(packet) => packet,
                _ => unreachable!(),
            };
            assert_eq!(client_public_key, packet.client_public_key);
            assert_eq!(GAME_ID, packet.game_id);
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connecting_should_time_out() {
    run_test_with_timeout(async move {
        let InitWithConnectingConnection {
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_connecting_connection();

        assert_eq!(Event::Disconnected, events.recv().await.unwrap());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn responding_should_send_connectresponses() {
    run_test_with_timeout(async move {
        let InitWithRespondingConnection {
            network,
            cancel_guard: _cancel_guard,
            shared_secret,
            token,
            ..
        } = init_with_responding_connection();

        let socket = network.connect(FakeAddr::Server, FakeAddr::Client);
        for _ in 0..3 {
            let packet = match recv_packet(&socket).await {
                Packet::ConnectResponse(packet) => packet,
                _ => unreachable!(),
            };
            let received_token = packet.open(&shared_secret).unwrap();
            assert_eq!(token, received_token);
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn responding_should_time_out() {
    run_test_with_timeout(async move {
        let InitWithRespondingConnection {
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_responding_connection();

        assert_eq!(Event::Disconnected, events.recv().await.unwrap());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_should_send_keepalives() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            shared_secret,
            ..
        } = init_with_connected_connection();

        let socket = network.connect(FakeAddr::Server, FakeAddr::Client);
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
async fn connected_should_time_out() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_connected_connection();

        assert_eq!(Event::Disconnected, events.recv().await.unwrap());
    })
    .await;
}
