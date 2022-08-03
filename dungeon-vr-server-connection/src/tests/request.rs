use std::num::NonZeroU8;
use std::time::Duration;

use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_shared::net_game::PlayerId;
use dungeon_vr_stream_codec::UnframedByteVec;
use tokio::time::sleep;

use crate::testing::{
    init_with_connected_connection, recv_packet, run_test_with_timeout, FakeAddr,
    InitWithConnectedConnection,
};
use crate::Request;

#[tokio::test(start_paused = true)]
async fn connected_request_gamedata_should_send_gamedata() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            requests,
            shared_secret,
            ..
        } = init_with_connected_connection();
        let socket = network.bind(FakeAddr::Client1);

        requests
            .send(Request::SendGameData {
                player_id: PlayerId(NonZeroU8::new(1).unwrap()),
                data: b"abcdef".to_vec(),
            })
            .await
            .unwrap();

        let sealed = match recv_packet(&socket).await {
            Packet::GameData(sealed) => sealed,
            _ => unreachable!(),
        };
        let data = sealed.open_ext::<UnframedByteVec>(&shared_secret).unwrap();
        assert_eq!(b"abcdef", data.as_slice());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn connected_request_gamedata_should_refresh_keepalive() {
    run_test_with_timeout(async move {
        let InitWithConnectedConnection {
            network,
            cancel_guard: _cancel_guard,
            requests,
            shared_secret,
            ..
        } = init_with_connected_connection();
        let socket = network.bind(FakeAddr::Client1);

        sleep(Duration::from_millis(900)).await;
        requests
            .send(Request::SendGameData {
                player_id: PlayerId(NonZeroU8::new(1).unwrap()),
                data: b"abcdef".to_vec(),
            })
            .await
            .unwrap();
        sleep(Duration::from_millis(900)).await;

        let sealed = match recv_packet(&socket).await {
            Packet::GameData(sealed) => sealed,
            _ => unreachable!(),
        };
        let data = sealed.open_ext::<UnframedByteVec>(&shared_secret).unwrap();
        assert_eq!(b"abcdef", data.as_slice());
    })
    .await;
}
