use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt::{self, Debug, Display, Formatter};
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::time::Duration;

use dungeon_vr_cryptography::{PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_shared::cancel;
use dungeon_vr_shared::net_game::{ClientId, NetGame};
use dungeon_vr_shared::protocol::challenge_token::ChallengeToken;
use dungeon_vr_shared::protocol::packet::Packet;
use dungeon_vr_shared::protocol::SAFE_RECV_BUFFER_SIZE;
use dungeon_vr_socket::testing::{FakeNetwork, FakeSocket};
use dungeon_vr_socket::BoundSocket;
use dungeon_vr_stream_codec::StreamCodec;
use tokio::time::error::Elapsed;
use tokio::time::{interval, sleep, timeout};
use tokio::try_join;

use crate::{
    ClientIdAllocator, ConnectedConnection, Connection, ConnectionVariant, InnerServer,
    PendingConnection, TickId, CLIENT_TIMEOUT_INTERVAL, CONNECT_CHALLENGE_INTERVAL, TICK_INTERVAL,
};

pub async fn box_err<T, E>(f: impl Future<Output = Result<T, E>>) -> Result<T, Box<dyn Error>>
where
    E: Error + 'static,
{
    f.await.map_err(|e| Box::new(e) as Box<dyn Error>)
}

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

pub(crate) async fn run_server_and_test_with_timeout<S>(
    server: InnerServer<S>,
    f: impl Future<Output = ()> + Send + 'static,
) where
    S: BoundSocket + Send + Sync + 'static,
{
    let cancel_token = server.cancel.clone();
    let server_run = tokio::spawn(server.run());
    let verification = timeout(
        Duration::from_secs(60),
        tokio::spawn(async move {
            f.await;
            cancel_token.cancel();
        }),
    );
    try_join!(box_err(server_run), box_deadline_err(verification)).unwrap();
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

pub(crate) fn make_network_and_inner_server(
) -> (FakeNetwork<FakeAddr>, InnerServer<FakeSocket<FakeAddr>>) {
    let network = FakeNetwork::new();
    let server = InnerServer {
        cancel: cancel::Token::new(),
        socket: network.bind(FakeAddr::Server),
        game: NetGame::new(),
        connections: HashMap::new(),
        client_ids: ClientIdAllocator::new(),
        tick_interval: interval(TICK_INTERVAL),
        tick_id: TickId(0),
    };
    (network, server)
}

pub(crate) struct InitWithPendingConnection {
    pub(crate) network: FakeNetwork<FakeAddr>,
    pub(crate) server: InnerServer<FakeSocket<FakeAddr>>,
    pub(crate) client_private_key: PrivateKey,
    pub(crate) server_public_key: PublicKey,
    pub(crate) shared_secret: SharedSecret,
    pub(crate) token: ChallengeToken,
}

pub(crate) fn init_with_pending_connection() -> InitWithPendingConnection {
    let (network, mut server) = make_network_and_inner_server();
    let client_private_key = PrivateKey::gen();
    let client_public_key = client_private_key.to_public();
    let server_private_key = PrivateKey::gen();
    let server_public_key = server_private_key.to_public();
    let shared_secret = server_private_key.exchange(&client_public_key).unwrap();
    let token = ChallengeToken::gen();
    server.connections.insert(
        FakeAddr::Client1,
        Connection {
            shared_secret,
            timeout: Some(Box::pin(sleep(CLIENT_TIMEOUT_INTERVAL))),
            variant: ConnectionVariant::Pending(PendingConnection {
                server_public_key,
                token,
                send_interval: interval(CONNECT_CHALLENGE_INTERVAL),
            }),
            _phantom_socket: PhantomData,
        },
    );
    InitWithPendingConnection {
        network,
        server,
        client_private_key,
        server_public_key,
        shared_secret,
        token,
    }
}

pub(crate) struct InitWithConnectedConnection {
    pub(crate) network: FakeNetwork<FakeAddr>,
    pub(crate) server: InnerServer<FakeSocket<FakeAddr>>,
    pub(crate) shared_secret: SharedSecret,
}

pub(crate) fn init_with_connected_connection() -> InitWithConnectedConnection {
    let (network, mut server) = make_network_and_inner_server();
    let shared_secret = SharedSecret::gen();
    server.connections.insert(
        FakeAddr::Client1,
        Connection {
            shared_secret,
            // Less than the maximum so that timeout refreshes can be detected.
            timeout: Some(Box::pin(sleep(Duration::from_millis(2500)))),
            variant: ConnectionVariant::Connected(ConnectedConnection {
                client_id: ClientId(NonZeroU8::new(1).unwrap()),
                input_buffer: BTreeMap::new(),
                // Less than the maximum so that keepalive refreshes can be detected.
                keepalive: Box::pin(sleep(Duration::from_millis(500))),
            }),
            _phantom_socket: PhantomData,
        },
    );
    InitWithConnectedConnection {
        network,
        server,
        shared_secret,
    }
}
