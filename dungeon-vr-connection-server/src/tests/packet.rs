use std::time::Duration;

use dungeon_vr_connection_shared::challenge_token::ChallengeToken;
use dungeon_vr_connection_shared::connect_init_packet::ConnectInitPacket;
use dungeon_vr_connection_shared::packet::Packet;
use dungeon_vr_connection_shared::sealed::Sealed;
use dungeon_vr_connection_shared::GAME_ID;
use dungeon_vr_cryptography::{PrivateKey, SharedSecret};
use dungeon_vr_stream_codec::UnframedByteVec;
use tokio::time::sleep;

use crate::testing::{
    init, init_with_connected_connection, init_with_pending_connection, run_test_with_timeout,
    send_bytes_to, send_packet_to, FakeAddr, InitWithConnectedConnection,
    InitWithPendingConnection,
};
use crate::{ConnectionState, Event};

#[tokio::test(start_paused = true)]
async fn no_connection_recv_empty_should_ignore() {
    run_test_with_timeout(async move {
        let (network, _cancel_guard, _requests, mut events) = init();

        let socket = network.bind(FakeAddr::Client1);
        send_bytes_to(&socket, b"", FakeAddr::Server).await;

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn no_connection_recv_malformed_should_ignore() {
    run_test_with_timeout(async move {
        let (network, _cancel_guard, _requests, mut events) = init();

        let socket = network.bind(FakeAddr::Client1);
        send_bytes_to(&socket, b"\x01ConnectInit but too short", FakeAddr::Server).await;

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn no_connection_recv_connectinit_should_create_pending_connection() {
    run_test_with_timeout(async move {
        let (network, _cancel_guard, _requests, mut events) = init();

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

        assert_eq!(
            Event::State {
                addr: FakeAddr::Client1,
                state: ConnectionState::Pending,
            },
            events.recv().await.unwrap()
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_connectinit_should_ignore() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
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

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_connectresponse_should_connect() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
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

        assert_eq!(
            Event::State {
                addr: FakeAddr::Client1,
                state: ConnectionState::Connected,
            },
            events.recv().await.unwrap(),
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_bad_signature_connectresponse_should_discard() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
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

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn pending_connection_recv_bad_token_connectresponse_should_discard() {
    run_test_with_timeout(async move {
        let InitWithPendingConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
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

        sleep(Duration::from_millis(500)).await;
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_recv_keepalive_should_refresh_timeout() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_connected_connection();

        sleep(Duration::from_millis(4900)).await;
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::Keepalive(Sealed::seal((), &shared_secret)),
            FakeAddr::Server,
        )
        .await;
        sleep(Duration::from_millis(4900)).await;

        // If the timeout had not been refreshed, the connection would have timed out, generating an
        // event.
        assert!(events.try_recv().is_err());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_recv_gamedata_should_yield_event() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_connected_connection();

        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::GameData(
                Sealed::seal_ext::<UnframedByteVec>(b"abcdef".to_vec(), &shared_secret).cast(),
            ),
            FakeAddr::Server,
        )
        .await;

        assert_eq!(
            Event::GameData {
                addr: FakeAddr::Client1,
                data: b"abcdef".to_vec()
            },
            events.recv().await.unwrap()
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_connection_recv_gamedata_should_refresh_timeout() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            mut events,
            shared_secret,
            ..
        } = init_with_connected_connection();

        sleep(Duration::from_millis(4900)).await;
        let socket = network.bind(FakeAddr::Client1);
        send_packet_to(
            &socket,
            Packet::GameData(
                Sealed::seal_ext::<UnframedByteVec>(b"abcdef".to_vec(), &shared_secret).cast(),
            ),
            FakeAddr::Server,
        )
        .await;
        sleep(Duration::from_millis(4900)).await;

        assert_eq!(
            Event::GameData {
                addr: FakeAddr::Client1,
                data: b"abcdef".to_vec(),
            },
            events.recv().await.unwrap()
        );
        // If the timeout had not been refreshed, the connection would have timed out, generating an
        // event.
        assert!(events.try_recv().is_err());
    })
    .await;
}
