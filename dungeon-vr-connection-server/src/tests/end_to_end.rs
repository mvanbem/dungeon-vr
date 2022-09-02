use dungeon_vr_connection_shared::connect_init_packet::ConnectInitPacket;
use dungeon_vr_connection_shared::packet::Packet;
use dungeon_vr_connection_shared::sealed::Sealed;
use dungeon_vr_connection_shared::GAME_ID;
use dungeon_vr_cryptography::PrivateKey;
use dungeon_vr_socket::testing::FakeNetwork;

use crate::testing::{recv_packet, run_test_with_timeout, send_packet_to, FakeAddr};
use crate::{ConnectionServer, ConnectionState, Event};

#[tokio::test(start_paused = true)]
async fn end_to_end() {
    run_test_with_timeout(async move {
        let network = FakeNetwork::new();
        let (cancel_guard, _requests, mut events) =
            ConnectionServer::spawn(Box::new(network.bind(FakeAddr::Server)));
        let socket = network.bind(FakeAddr::Client1);

        // Send a ConnectInit packet.
        let client_private_key = PrivateKey::gen();
        let client_public_key = client_private_key.to_public();
        send_packet_to(
            &socket,
            Packet::ConnectInit(ConnectInitPacket {
                game_id: GAME_ID,
                client_public_key,
            }),
            FakeAddr::Server,
        )
        .await;
        assert_eq!(
            Event::State {
                addr: FakeAddr::Client1,
                state: ConnectionState::Pending
            },
            events.recv().await.unwrap()
        );

        println!("Waiting for a ConnectChallenge packet");
        let packet = match recv_packet(&socket).await {
            Packet::ConnectChallenge(packet) => packet,
            _ => unreachable!(),
        };
        let shared_secret = client_private_key
            .exchange(&packet.server_public_key)
            .unwrap();
        let token = packet.sealed_payload.open(&shared_secret).unwrap();

        // Send a ConnectResponse packet.
        send_packet_to(
            &socket,
            Packet::ConnectResponse(Sealed::seal(token, &shared_secret)),
            FakeAddr::Server,
        )
        .await;
        assert_eq!(
            Event::State {
                addr: FakeAddr::Client1,
                state: ConnectionState::Connected
            },
            events.recv().await.unwrap(),
        );

        println!("Waiting for a Keepalive packet");
        let packet = match recv_packet(&socket).await {
            Packet::Keepalive(packet) => packet,
            _ => unreachable!(),
        };
        let () = packet.open(&shared_secret).unwrap();

        // Send a Disconnect packet.
        send_packet_to(
            &socket,
            Packet::Disconnect(Sealed::seal((), &shared_secret)),
            FakeAddr::Server,
        )
        .await;
        assert_eq!(
            Event::State {
                addr: FakeAddr::Client1,
                state: ConnectionState::Disconnected
            },
            events.recv().await.unwrap(),
        );

        drop(cancel_guard);
        assert_eq!(Event::Dropped, events.recv().await.unwrap());
        assert!(events.recv().await.is_none());
    })
    .await;
}
