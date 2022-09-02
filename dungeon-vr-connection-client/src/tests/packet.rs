use std::time::Duration;

use dungeon_vr_connection_shared::challenge_token::ChallengeToken;
use dungeon_vr_connection_shared::connect_challenge_packet::ConnectChallengePacket;
use dungeon_vr_connection_shared::packet::Packet;
use dungeon_vr_connection_shared::sealed::Sealed;
use dungeon_vr_cryptography::{PrivateKey, SharedSecret};
use dungeon_vr_stream_codec::UnframedByteVec;
use tokio::time::sleep;

use crate::testing::{
    init_with_connected_connection, init_with_connecting_connection,
    init_with_responding_connection, run_test_with_timeout, send_bytes, send_packet, FakeAddr,
    InitWithConnectedConnection, InitWithConnectingConnection, InitWithRespondingConnection,
};
use crate::{ConnectionState, Event};

#[tokio::test(start_paused = true)]
async fn connecting_recv_empty_should_ignore() {
    run_test_with_timeout(async move {
        let InitWithConnectingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_connecting_connection();

        let socket = network.bind(FakeAddr::Server);
        send_bytes(&socket, b"").await;

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connecting_recv_malformed_should_ignore() {
    run_test_with_timeout(async move {
        let InitWithConnectingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_connecting_connection();

        let socket = network.bind(FakeAddr::Server);
        send_bytes(&socket, b"\x02ConnectChallenge but too short").await;

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connecting_recv_challenge_should_change_state() {
    run_test_with_timeout(async move {
        let InitWithConnectingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            client_public_key,
            ..
        } = init_with_connecting_connection();

        let socket = network.bind(FakeAddr::Server);
        let server_private_key = PrivateKey::gen();
        let server_public_key = server_private_key.to_public();
        let shared_secret = server_private_key.exchange(&client_public_key).unwrap();
        let token = ChallengeToken::gen();
        send_packet(
            &socket,
            Packet::ConnectChallenge(ConnectChallengePacket {
                server_public_key,
                sealed_payload: Sealed::seal(token, &shared_secret),
            }),
        )
        .await;

        assert_eq!(
            Event::State(ConnectionState::Responding),
            events.recv().await.unwrap()
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connecting_recv_bad_signature_challenge_should_ignore() {
    run_test_with_timeout(async move {
        let InitWithConnectingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            ..
        } = init_with_connecting_connection();

        let socket = network.bind(FakeAddr::Server);
        send_packet(
            &socket,
            Packet::ConnectChallenge(ConnectChallengePacket {
                server_public_key: PrivateKey::gen().to_public(),
                sealed_payload: Sealed::seal(ChallengeToken::gen(), &SharedSecret::gen()),
            }),
        )
        .await;

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn responding_recv_keepalive_should_change_state() {
    run_test_with_timeout(async move {
        let InitWithRespondingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_responding_connection();

        let socket = network.bind(FakeAddr::Server);
        send_packet(&socket, Packet::Keepalive(Sealed::seal((), &shared_secret))).await;

        assert_eq!(
            Event::State(ConnectionState::Connected),
            events.recv().await.unwrap()
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn responding_recv_gamedata_should_change_state() {
    run_test_with_timeout(async move {
        let InitWithRespondingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_responding_connection();

        let socket = network.bind(FakeAddr::Server);
        send_packet(
            &socket,
            Packet::GameData(Sealed::seal_ext::<UnframedByteVec>(
                b"abcdef".to_vec(),
                &shared_secret,
            )),
        )
        .await;

        assert_eq!(
            Event::State(ConnectionState::Connected),
            events.recv().await.unwrap()
        );
        assert_eq!(
            Event::GameData(b"abcdef".to_vec()),
            events.recv().await.unwrap()
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_recv_keepalive_should_refresh_timeout() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_connected_connection();

        sleep(Duration::from_millis(4900)).await;
        let socket = network.bind(FakeAddr::Server);
        send_packet(&socket, Packet::Keepalive(Sealed::seal((), &shared_secret))).await;
        sleep(Duration::from_millis(4900)).await;

        // If the timeout had not been refreshed, the connection would have timed out, generating an
        // event.
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_recv_gamedata_should_yield_event() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_connected_connection();

        let socket = network.bind(FakeAddr::Server);
        send_packet(
            &socket,
            Packet::GameData(Sealed::seal_ext::<UnframedByteVec>(
                b"abcdef".to_vec(),
                &shared_secret,
            )),
        )
        .await;

        assert_eq!(
            Event::GameData(b"abcdef".to_vec()),
            events.recv().await.unwrap()
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_recv_gamedata_should_refresh_timeout() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_connected_connection();

        sleep(Duration::from_millis(4900)).await;
        let socket = network.bind(FakeAddr::Server);
        send_packet(
            &socket,
            Packet::GameData(Sealed::seal_ext::<UnframedByteVec>(
                b"abcdef".to_vec(),
                &shared_secret,
            )),
        )
        .await;
        sleep(Duration::from_millis(4900)).await;

        assert_eq!(
            Event::GameData(b"abcdef".to_vec()),
            events.recv().await.unwrap()
        );
        // If the timeout had not been refreshed, the connection would have timed out, generating an
        // event.
        assert!(events.try_recv().is_err());
    })
    .await;
}
