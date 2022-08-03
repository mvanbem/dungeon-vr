use dungeon_vr_cryptography::PrivateKey;
use dungeon_vr_protocol::challenge_token::ChallengeToken;
use dungeon_vr_protocol::connect_challenge_packet::ConnectChallengePacket;
use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_protocol::sealed::Sealed;
use dungeon_vr_protocol::GAME_ID;
use dungeon_vr_socket::testing::FakeNetwork;

use crate::testing::{recv_packet, run_test_with_timeout, send_packet, FakeAddr};
use crate::{ClientConnection, Event};

#[tokio::test(start_paused = true)]
async fn end_to_end() {
    run_test_with_timeout(async move {
        let network = FakeNetwork::new();
        let (cancel_guard, _requests, mut events) =
            ClientConnection::spawn(network.connect(FakeAddr::Client, FakeAddr::Server));
        let socket = network.connect(FakeAddr::Server, FakeAddr::Client);

        println!("Waiting for a ConnectInit packet");
        let packet = recv_packet(&socket).await;
        let packet = match packet {
            Packet::ConnectInit(packet) => packet,
            _ => panic!(),
        };
        assert_eq!(GAME_ID, packet.game_id);
        let client_public_key = packet.client_public_key;

        // Send a ConnectChallenge packet.
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
        assert_eq!(Event::Responding, events.recv().await.unwrap());

        println!("Waiting for a ConnectResponse packet");
        let packet = recv_packet(&socket).await;
        let response_token = match packet {
            Packet::ConnectResponse(packet) => packet.open(&shared_secret).unwrap(),
            _ => panic!(),
        };
        assert_eq!(token, response_token);

        // Send a Keepalive packet.
        send_packet(&socket, Packet::Keepalive(Sealed::seal((), &shared_secret))).await;
        assert_eq!(Event::Connected, events.recv().await.unwrap());

        println!("Waiting for a Keepalive packet");
        let packet = match recv_packet(&socket).await {
            Packet::Keepalive(packet) => packet,
            _ => unreachable!(),
        };
        let () = packet.open(&shared_secret).unwrap();

        // Send a Disconnect packet.
        send_packet(
            &socket,
            Packet::Disconnect(Sealed::seal((), &shared_secret)),
        )
        .await;
        assert_eq!(Event::Disconnected, events.recv().await.unwrap());

        drop(cancel_guard);
        assert_eq!(Event::Dropped, events.recv().await.unwrap());
        assert!(events.recv().await.is_none());
    })
    .await;
}
