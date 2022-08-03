use std::error::Error;
use std::fmt::{self, Debug, Display, Formatter};
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::time::Duration;

use dungeon_vr_cryptography::{PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_protocol::challenge_token::ChallengeToken;
use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_protocol::SAFE_RECV_BUFFER_SIZE;
use dungeon_vr_shared::cancel;
use dungeon_vr_shared::net_game::PlayerId;
use dungeon_vr_socket::testing::{FakeBoundSocket, FakeNetwork};
use dungeon_vr_socket::BoundSocket;
use dungeon_vr_stream_codec::StreamCodec;
use tokio::sync::mpsc;
use tokio::time::error::Elapsed;
use tokio::time::{interval, sleep, timeout};

use crate::{
    ConnectedConnection, Connection, ConnectionVariant, DisconnectingConnection, Event,
    PendingConnection, Request, ServerConnection, CLIENT_TIMEOUT_INTERVAL, DISCONNECT_PACKET_COUNT,
    EVENT_BUFFER_SIZE, KEEPALIVE_INTERVAL, REQUEST_BUFFER_SIZE, SEND_INTERVAL,
};

pub async fn box_deadline_err<T, E>(
    f: impl Future<Output = Result<Result<T, E>, Elapsed>>,
) -> Result<T, Box<dyn Error>>
where
    E: Error + 'static,
{
    match f.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(Box::new(e) as Box<dyn Error>),
        Err(e) => Err(Box::new(e) as Box<dyn Error>),
    }
}

pub async fn run_test_with_timeout(f: impl Future<Output = ()> + Send + 'static) {
    box_deadline_err(timeout(Duration::from_secs(60), tokio::spawn(f)))
        .await
        .unwrap();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FakeAddr {
    Server,
    Client1,
}

impl Display for FakeAddr {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

pub async fn recv_packet<S: BoundSocket>(socket: &S) -> Packet {
    let mut buf = [0; SAFE_RECV_BUFFER_SIZE];
    let (size, _) = socket.recv_from(&mut buf).await.unwrap();
    let mut r = &buf[..size];
    let packet = Packet::read_from(&mut r).unwrap();
    assert!(r.is_empty());
    packet
}

pub async fn send_bytes_to<S: BoundSocket>(socket: &S, buf: &[u8], addr: <S as BoundSocket>::Addr) {
    socket.send_to(buf, addr).await.unwrap();
}

pub async fn send_packet_to<S: BoundSocket>(
    socket: &S,
    packet: Packet,
    addr: <S as BoundSocket>::Addr,
) {
    let mut buf = Vec::new();
    packet.write_to(&mut buf).unwrap();
    socket.send_to(&buf, addr).await.unwrap();
}

fn make_network_and_connection(
    mutate_connection: impl FnOnce(&mut ServerConnection<FakeBoundSocket<FakeAddr>>),
) -> (
    FakeNetwork<FakeAddr>,
    cancel::Guard,
    mpsc::Sender<Request>,
    mpsc::Receiver<Event>,
) {
    let network = FakeNetwork::new();
    let socket = network.bind(FakeAddr::Server);
    let cancel_token = cancel::Token::new();
    let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
    let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER_SIZE);

    let mut connection = ServerConnection::new(socket, request_rx, event_tx);
    mutate_connection(&mut connection);
    tokio::spawn(connection.run(cancel_token.clone()));

    (network, cancel_token.guard(), request_tx, event_rx)
}

pub fn init() -> (
    FakeNetwork<FakeAddr>,
    cancel::Guard,
    mpsc::Sender<Request>,
    mpsc::Receiver<Event>,
) {
    make_network_and_connection(|_| ())
}

pub struct InitWithPendingConnection {
    pub network: FakeNetwork<FakeAddr>,
    pub cancel_guard: cancel::Guard,
    pub requests: mpsc::Sender<Request>,
    pub events: mpsc::Receiver<Event>,
    pub client_private_key: PrivateKey,
    pub server_public_key: PublicKey,
    pub shared_secret: SharedSecret,
    pub token: ChallengeToken,
}

pub fn init_with_pending_connection() -> InitWithPendingConnection {
    let client_private_key = PrivateKey::gen();
    let client_public_key = client_private_key.to_public();
    let server_private_key = PrivateKey::gen();
    let server_public_key = server_private_key.to_public();
    let shared_secret = server_private_key.exchange(&client_public_key).unwrap();
    let token = ChallengeToken::gen();
    let (network, cancel_guard, requests, events) = make_network_and_connection(|connection| {
        connection.connections.insert(
            FakeAddr::Client1,
            Connection {
                shared_secret,
                timeout: Some(Box::pin(sleep(CLIENT_TIMEOUT_INTERVAL))),
                variant: ConnectionVariant::Pending(PendingConnection {
                    server_public_key,
                    token,
                    send_interval: interval(SEND_INTERVAL),
                }),
                _phantom_socket: PhantomData,
            },
        );
    });
    InitWithPendingConnection {
        network,
        cancel_guard,
        requests,
        events,
        client_private_key,
        server_public_key,
        shared_secret,
        token,
    }
}

pub struct InitWithConnectedConnection {
    pub network: FakeNetwork<FakeAddr>,
    pub cancel_guard: cancel::Guard,
    pub requests: mpsc::Sender<Request>,
    pub events: mpsc::Receiver<Event>,
    pub shared_secret: SharedSecret,
}

pub fn init_with_connected_connection() -> InitWithConnectedConnection {
    let shared_secret = SharedSecret::gen();
    let (network, cancel_guard, requests, events) = make_network_and_connection(|connection| {
        connection.connections.insert(
            FakeAddr::Client1,
            Connection {
                shared_secret,
                timeout: Some(Box::pin(sleep(CLIENT_TIMEOUT_INTERVAL))),
                variant: ConnectionVariant::Connected(ConnectedConnection {
                    player_id: PlayerId(NonZeroU8::new(1).unwrap()),
                    keepalive: Box::pin(sleep(KEEPALIVE_INTERVAL)),
                }),
                _phantom_socket: PhantomData,
            },
        );
    });
    InitWithConnectedConnection {
        network,
        cancel_guard,
        requests,
        events,
        shared_secret,
    }
}

pub struct InitWithDisconnectingConnection {
    pub network: FakeNetwork<FakeAddr>,
    pub cancel_guard: cancel::Guard,
    pub requests: mpsc::Sender<Request>,
    pub events: mpsc::Receiver<Event>,
    pub shared_secret: SharedSecret,
}

pub fn init_with_disconnecting_connection() -> InitWithDisconnectingConnection {
    let shared_secret = SharedSecret::gen();
    let (network, cancel_guard, requests, events) = make_network_and_connection(|connection| {
        connection.connections.insert(
            FakeAddr::Client1,
            Connection {
                shared_secret,
                timeout: Some(Box::pin(sleep(CLIENT_TIMEOUT_INTERVAL))),
                variant: ConnectionVariant::Disconnecting(DisconnectingConnection {
                    interval: interval(SEND_INTERVAL),
                    packets_to_send: DISCONNECT_PACKET_COUNT,
                }),
                _phantom_socket: PhantomData,
            },
        );
    });
    InitWithDisconnectingConnection {
        network,
        cancel_guard,
        requests,
        events,
        shared_secret,
    }
}
