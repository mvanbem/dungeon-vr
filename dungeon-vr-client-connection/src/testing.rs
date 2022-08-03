use std::error::Error;
use std::fmt::{self, Debug, Display, Formatter};
use std::future::Future;
use std::time::Duration;

use dungeon_vr_cryptography::{PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_protocol::challenge_token::ChallengeToken;
use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_protocol::SAFE_RECV_BUFFER_SIZE;
use dungeon_vr_shared::cancel;
use dungeon_vr_socket::testing::{FakeConnectedSocket, FakeNetwork};
use dungeon_vr_socket::ConnectedSocket;
use dungeon_vr_stream_codec::StreamCodec;
use tokio::sync::mpsc;
use tokio::time::error::Elapsed;
use tokio::time::{interval, sleep, timeout};

use crate::{
    ClientConnection, Event, Request, Variant, CONNECTING_RESPONDING_SEND_INTERVAL,
    EVENT_BUFFER_SIZE, KEEPALIVE_INTERVAL, REQUEST_BUFFER_SIZE, SERVER_TIMEOUT_INTERVAL,
};

async fn box_deadline_err<T, E>(
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
    Client,
    Server,
}

impl Display for FakeAddr {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        <Self as Debug>::fmt(self, f)
    }
}

pub async fn recv_packet<S: ConnectedSocket>(socket: &S) -> Packet {
    let mut buf = [0; SAFE_RECV_BUFFER_SIZE];
    let size = socket.recv(&mut buf).await.unwrap();
    let mut r = &buf[..size];
    let packet = Packet::read_from(&mut r).unwrap();
    assert!(r.is_empty());
    packet
}

pub async fn send_bytes<S: ConnectedSocket>(socket: &S, buf: &[u8]) {
    socket.send(buf).await.unwrap();
}

pub async fn send_packet<S: ConnectedSocket>(socket: &S, packet: Packet) {
    let mut buf = Vec::new();
    packet.write_to(&mut buf).unwrap();
    socket.send(&buf).await.unwrap();
}

fn make_network_and_connection(
    mutate_connection: impl FnOnce(&mut ClientConnection<FakeConnectedSocket<FakeAddr>>),
) -> (
    FakeNetwork<FakeAddr>,
    cancel::Guard,
    mpsc::Sender<Request>,
    mpsc::Receiver<Event>,
) {
    let network = FakeNetwork::new();
    let socket = network.connect(FakeAddr::Client, FakeAddr::Server);
    let cancel_token = cancel::Token::new();
    let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
    let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER_SIZE);

    let mut connection = ClientConnection::new(socket, request_rx, event_tx);
    mutate_connection(&mut connection);
    tokio::spawn(connection.run(cancel_token.clone()));

    (network, cancel_token.guard(), request_tx, event_rx)
}

pub struct InitWithConnectingConnection {
    pub network: FakeNetwork<FakeAddr>,
    pub cancel_guard: cancel::Guard,
    pub requests: mpsc::Sender<Request>,
    pub events: mpsc::Receiver<Event>,
    pub client_private_key: PrivateKey,
    pub client_public_key: PublicKey,
}

pub fn init_with_connecting_connection() -> InitWithConnectingConnection {
    let client_private_key = PrivateKey::gen();
    let client_public_key = client_private_key.to_public();
    let (network, cancel_guard, requests, events) = make_network_and_connection(|connection| {
        connection.variant = Variant::Connecting {
            client_private_key: client_private_key.clone(),
            client_public_key,
            send_interval: interval(CONNECTING_RESPONDING_SEND_INTERVAL),
        };
    });
    InitWithConnectingConnection {
        network,
        cancel_guard,
        requests,
        events,
        client_private_key,
        client_public_key,
    }
}

pub struct InitWithRespondingConnection {
    pub network: FakeNetwork<FakeAddr>,
    pub cancel_guard: cancel::Guard,
    pub requests: mpsc::Sender<Request>,
    pub events: mpsc::Receiver<Event>,
    pub shared_secret: SharedSecret,
    pub token: ChallengeToken,
}

pub fn init_with_responding_connection() -> InitWithRespondingConnection {
    let shared_secret = SharedSecret::gen();
    let token = ChallengeToken::gen();
    let (network, cancel_guard, requests, events) = make_network_and_connection(|connection| {
        connection.variant = Variant::Responding {
            shared_secret,
            token,
            send_interval: interval(CONNECTING_RESPONDING_SEND_INTERVAL),
        };
    });
    InitWithRespondingConnection {
        network,
        cancel_guard,
        requests,
        events,
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
        connection.timeout = Some(Box::pin(sleep(SERVER_TIMEOUT_INTERVAL)));
        connection.variant = Variant::Connected {
            shared_secret,
            keepalive: Box::pin(sleep(KEEPALIVE_INTERVAL)),
        };
    });
    InitWithConnectedConnection {
        network,
        cancel_guard,
        requests,
        events,
        shared_secret,
    }
}
