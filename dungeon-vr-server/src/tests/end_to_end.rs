use std::time::Duration;

use dungeon_vr_cryptography::PrivateKey;
use dungeon_vr_shared::protocol::connect_init_packet::ConnectInitPacket;
use dungeon_vr_shared::protocol::packet::Packet;
use dungeon_vr_shared::protocol::sealed::Sealed;
use dungeon_vr_shared::protocol::GAME_ID;
use dungeon_vr_socket::testing::FakeNetwork;
use tokio::time::timeout;
use tokio::try_join;

use crate::testing::{box_deadline_err, box_err, recv_packet, send_packet_to, FakeAddr};
use crate::Server;

#[tokio::test(start_paused = true)]
async fn handshake_integration_test() {
    let network = FakeNetwork::new();
    let server = Server::spawn(network.bind(FakeAddr::Server));
    let cancel_token = server.cancel_token().clone();

    let verification = timeout(
        Duration::from_secs(60),
        tokio::spawn(async move {
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

            println!("Waiting for a Keepalive packet");
            let packet = match recv_packet(&socket).await {
                Packet::Keepalive(packet) => packet,
                _ => unreachable!(),
            };
            packet.open(&shared_secret).unwrap();

            cancel_token.cancel();
        }),
    );

    try_join!(box_err(server.join()), box_deadline_err(verification)).unwrap();
}
