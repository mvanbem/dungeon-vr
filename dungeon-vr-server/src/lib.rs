#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::future::pending;
use std::io;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::pin::Pin;

use dungeon_vr_cryptography::{KeyExchangeError, PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_shared::cancel;
use dungeon_vr_shared::net_game::{ClientId, Input, NetGame};
use dungeon_vr_shared::protocol::challenge_token::ChallengeToken;
use dungeon_vr_shared::protocol::connect_challenge_packet::ConnectChallengePacket;
use dungeon_vr_shared::protocol::connect_init_packet::ConnectInitPacket;
use dungeon_vr_shared::protocol::packet::Packet;
use dungeon_vr_shared::protocol::sealed::Sealed;
use dungeon_vr_shared::protocol::{GAME_ID, SAFE_RECV_BUFFER_SIZE};
use dungeon_vr_stream_codec::StreamCodec;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use tokio::select;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::{interval, sleep, Duration, Instant, Interval, Sleep};

use crate::socket::BoundSocket;

mod socket;

const TICK_INTERVAL: Duration = Duration::from_millis(50);

const DISCONNECT_INTERVAL: Duration = Duration::from_millis(250);
const DISCONNECT_PACKET_COUNT: usize = 10;
const CONNECT_CHALLENGE_INTERVAL: Duration = Duration::from_millis(250);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);
const CLIENT_TIMEOUT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TickId(u32);

pub struct Server {
    cancel_guard: cancel::Guard,
    join_handle: JoinHandle<()>,
}

impl Server {
    pub fn spawn<S: BoundSocket + Send + Sync + 'static>(socket: S) -> Self {
        let cancel = cancel::Token::new();
        let join_handle = tokio::spawn(InnerServer::new(cancel.clone(), socket).run());

        Self {
            cancel_guard: cancel.guard(),
            join_handle,
        }
    }

    pub async fn join(self) -> Result<(), JoinError> {
        self.join_handle.await?;
        Ok(())
    }

    pub async fn shutdown(self) -> Result<(), JoinError> {
        drop(self.cancel_guard);
        self.join_handle.await?;
        Ok(())
    }
}

struct InnerServer<S: BoundSocket> {
    cancel: cancel::Token,
    socket: S,
    game: NetGame,
    connections: HashMap<<S as BoundSocket>::Addr, Connection<S>>,
    client_ids: ClientIdAllocator,
    tick_interval: Interval,
    tick_id: TickId,
}

#[derive(Debug)]
enum Event<A> {
    SocketRecv(io::Result<(usize, A)>),
    Tick,
    ClientTimeout { peer: A },
    DisconnectElapsed { peer: A },
    PendingConnectionSend { peer: A },
    KeepaliveElapsed { peer: A },
}

impl<S: BoundSocket> InnerServer<S> {
    fn new(cancel: cancel::Token, socket: S) -> Self {
        Self {
            cancel,
            socket,
            game: NetGame::new(),
            connections: HashMap::new(),
            client_ids: ClientIdAllocator::new(),
            tick_interval: interval(TICK_INTERVAL),
            tick_id: TickId(0),
        }
    }

    async fn run(mut self) {
        let mut buf = [0; SAFE_RECV_BUFFER_SIZE];
        while !self.cancel.is_cancelled() {
            self.run_once(&mut buf).await;
        }
    }

    #[cfg(test)]
    async fn run_once_for_test(&mut self) {
        let mut buf = [0; SAFE_RECV_BUFFER_SIZE];
        self.run_once(&mut buf).await;
    }

    async fn run_once(&mut self, buf: &mut [u8; SAFE_RECV_BUFFER_SIZE]) {
        let mut dynamic_events: FuturesUnordered<_> = self
            .connections
            .iter_mut()
            .map(|(peer, connection)| connection.wait_for_event(*peer))
            .collect();

        let event = select! {
            biased;

            _ = self.cancel.cancelled() => return,

            result = self.socket.recv_from(buf) => Event::SocketRecv(result),

            _ = self.tick_interval.tick() => Event::Tick,

            Some(event) = dynamic_events.next() => event,
        };
        drop(dynamic_events);

        match event {
            Event::SocketRecv(Ok((size, peer))) => {
                self.handle_socket_recv(&buf[..size], peer);
            }
            Event::SocketRecv(Err(e)) => log::error!("Unexpected socket error: {e}"),
            Event::Tick => self.handle_tick(),
            Event::ClientTimeout { peer } => self.handle_client_timeout(peer),
            Event::DisconnectElapsed { peer } => self.handle_disconnect_elapsed(peer).await,
            Event::PendingConnectionSend { peer } => {
                self.handle_pending_connection_send(peer).await
            }
            Event::KeepaliveElapsed { peer } => self.handle_keepalive_elapsed(peer).await,
        }
    }

    fn handle_socket_recv(&mut self, mut r: &[u8], peer: <S as BoundSocket>::Addr) {
        let packet = match Packet::read_from(&mut r) {
            Ok(packet) => packet,
            Err(e) => {
                log::debug!("Peer {peer}: Dropping invalid packet: {e}");
                return;
            }
        };
        if r.len() > 0 {
            log::debug!(
                "Peer {peer}: Dropping {:?} packet: {} unexpected trailing byte(s)",
                packet.kind(),
                r.len(),
            );
            return;
        }
        match packet {
            Packet::Disconnect(packet) => return self.handle_disconnect_packet(peer, packet),
            Packet::ConnectInit(packet) => return self.handle_connect_init_packet(peer, packet),
            Packet::ConnectResponse(sealed) => self.handle_connect_response_packet(peer, sealed),
            _ => log::debug!(
                "Peer {peer}: Dropping unexpected {:?} packet",
                packet.kind(),
            ),
        }
    }

    fn handle_disconnect_packet(&mut self, peer: <S as BoundSocket>::Addr, packet: Sealed<()>) {
        let connection = match self.connections.get_mut(&peer) {
            Some(connection) => connection,
            None => {
                log::debug!("Peer {peer}: Dropping Disconnect packet: not connected");
                return;
            }
        };
        if let Err(e) = packet.open(&connection.shared_secret) {
            log::debug!("Peer {peer}: Dropping Disconnect packet: {e}");
            return;
        }

        log::info!("Peer {peer}: Disconnected");
        self.connections.remove(&peer);
    }

    fn handle_connect_init_packet(
        &mut self,
        peer: <S as BoundSocket>::Addr,
        packet: ConnectInitPacket,
    ) {
        if packet.game_id != GAME_ID {
            log::debug!(
                "Peer {peer}: Dropping ConnectInit paket: unsupported game ID 0x{:08x}",
                packet.game_id,
            );
            return;
        }
        if self.connections.contains_key(&peer) {
            log::debug!("Peer {peer}: Dropping redundant ConnectInit packet");
            return;
        }

        // Perform our side of the ECDH key exchange.
        let private_key = PrivateKey::gen();
        let server_public_key = private_key.to_public();
        let shared_secret = match private_key.exchange(&packet.client_public_key) {
            Ok(shared_secret) => shared_secret,
            Err(KeyExchangeError::NonContributory) => {
                log::debug!(
                    "Peer {peer}: Dropping ConnectInit packet: non-contributory key exchange"
                );
                return;
            }
        };

        // Record the new connection.
        let token = ChallengeToken::gen();
        self.connections.insert(
            peer,
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
        log::info!("Peer {peer}: New connection pending");
    }

    fn handle_connect_response_packet(
        &mut self,
        peer: <S as BoundSocket>::Addr,
        sealed: Sealed<ChallengeToken>,
    ) {
        let connection = match self.connections.get_mut(&peer) {
            Some(connection) => connection,
            None => {
                log::debug!("Peer {peer}: Dropping ConnectResponse packet: not connected");
                return;
            }
        };
        let packet_token = match sealed.open(&connection.shared_secret) {
            Ok(token) => token,
            Err(e) => {
                log::debug!("Peer {peer}: Dropping ConnectResponse packet: {e}");
                return;
            }
        };
        match connection.variant {
            ConnectionVariant::Pending(PendingConnection { token, .. }) => {
                if packet_token != token {
                    log::debug!(
                        "Peer {peer}: Dropping ConnectResponse packet: bad challenge token",
                    );
                    return;
                }
                // OK to proceed.
            }
            ConnectionVariant::Connected(_) => {
                log::debug!("Peer {peer}: Dropping redundant ConnectResponse packet");
                return;
            }
            ConnectionVariant::Disconnecting(_) => {
                log::debug!("Peer {peer}: Dropping ConnectResponse packet: disconnecting");
                return;
            }
        }

        // Advance this connection to the Connected state.
        let client_id = self.client_ids.allocate();
        connection.variant = ConnectionVariant::Connected(ConnectedConnection {
            client_id,
            input_buffer: BTreeMap::default(),
            keepalive: Box::pin(sleep(Duration::ZERO)),
        });
        log::info!("Peer {peer}: Connected");
    }

    fn handle_tick(&mut self) {
        // Gather the current buffered inputs for this tick from each connection.
        let mut player_inputs = BTreeMap::new();
        for connection in self.connections.values_mut() {
            if let ConnectionVariant::Connected(ConnectedConnection {
                client_id,
                input_buffer,
                ..
            }) = &mut connection.variant
            {
                if let Some(inputs) = input_buffer.remove(&self.tick_id) {
                    player_inputs.insert(*client_id, inputs);
                }
            }
        }
        self.game.update(player_inputs);

        // TODO: Send (delta) updates.

        self.tick_id = TickId(self.tick_id.0 + 1);
    }

    fn handle_client_timeout(&mut self, peer: <S as BoundSocket>::Addr) {
        let connection = self.connections.get_mut(&peer).unwrap();
        connection.timeout = None;
        connection.variant = ConnectionVariant::Disconnecting(DisconnectingConnection {
            interval: interval(DISCONNECT_INTERVAL),
            packets_to_send: DISCONNECT_PACKET_COUNT,
        });
        log::info!("Peer {peer}: Disconnecting (timed out)")
    }

    async fn handle_disconnect_elapsed(&mut self, peer: <S as BoundSocket>::Addr) {
        let connection = self.connections.get_mut(&peer).unwrap();
        let disconnecting = match &mut connection.variant {
            ConnectionVariant::Disconnecting(disconnecting) => disconnecting,
            _ => unreachable!(),
        };

        send_packet(
            &self.socket,
            peer,
            Packet::Disconnect(Sealed::seal((), &connection.shared_secret)),
        )
        .await;

        disconnecting.packets_to_send -= 1;
        if disconnecting.packets_to_send == 0 {
            self.connections.remove(&peer);
            log::debug!("Peer {peer}: Done sending Disconnect packets")
        }
    }

    async fn handle_pending_connection_send(&mut self, peer: <S as BoundSocket>::Addr) {
        let connection = self.connections.get_mut(&peer).unwrap();
        let pending = match &connection.variant {
            ConnectionVariant::Pending(pending) => pending,
            _ => unreachable!(),
        };
        send_packet(
            &self.socket,
            peer,
            Packet::ConnectChallenge(ConnectChallengePacket {
                server_public_key: pending.server_public_key,
                sealed_payload: Sealed::seal(pending.token, &connection.shared_secret),
            }),
        )
        .await;
    }

    async fn handle_keepalive_elapsed(&mut self, peer: <S as BoundSocket>::Addr) {
        let connection = self.connections.get_mut(&peer).unwrap();
        send_packet(
            &self.socket,
            peer,
            Packet::Keepalive(Sealed::seal((), &connection.shared_secret)),
        )
        .await;
        connection.refresh_keepalive();
    }
}

async fn send_packet<S, P>(socket: &S, peer: <S as BoundSocket>::Addr, packet: P)
where
    S: BoundSocket,
    P: StreamCodec<WriteError = Infallible>,
{
    let mut w = Vec::new();
    packet.write_to(&mut w).unwrap();
    if let Err(e) = socket.send_to(&w, peer).await {
        log::error!("Unexpected socket error: {e}");
    }
}

struct Connection<S> {
    shared_secret: SharedSecret,
    timeout: Option<Pin<Box<Sleep>>>,
    variant: ConnectionVariant,
    _phantom_socket: PhantomData<S>,
}

enum ConnectionVariant {
    Pending(PendingConnection),
    Connected(ConnectedConnection),
    Disconnecting(DisconnectingConnection),
}

struct PendingConnection {
    server_public_key: PublicKey,
    token: ChallengeToken,
    send_interval: Interval,
}

struct ConnectedConnection {
    client_id: ClientId,
    input_buffer: BTreeMap<TickId, Vec<Input>>,
    keepalive: Pin<Box<Sleep>>,
}

struct DisconnectingConnection {
    interval: Interval,
    packets_to_send: usize,
}

impl<S> Connection<S>
where
    S: BoundSocket,
{
    async fn wait_for_event(
        &mut self,
        peer: <S as BoundSocket>::Addr,
    ) -> Event<<S as BoundSocket>::Addr> {
        let timeout = match &mut self.timeout {
            Some(timeout) => timeout.left_future(),
            None => pending().right_future(),
        };
        let (pending_send, keepalive_elapsed, disconnect_elapsed) = match &mut self.variant {
            ConnectionVariant::Pending(PendingConnection { send_interval, .. }) => (
                send_interval.tick().left_future(),
                pending().right_future(),
                pending().right_future(),
            ),
            ConnectionVariant::Connected(ConnectedConnection { keepalive, .. }) => (
                pending().right_future(),
                keepalive.left_future(),
                pending().right_future(),
            ),
            ConnectionVariant::Disconnecting(DisconnectingConnection { interval, .. }) => (
                pending().right_future(),
                pending().right_future(),
                interval.tick().left_future(),
            ),
        };

        select! {
            biased;

            _ = pending_send => Event::PendingConnectionSend { peer },

            _ = keepalive_elapsed => Event::KeepaliveElapsed { peer },

            _ = disconnect_elapsed => Event::DisconnectElapsed { peer },

            _ = timeout => Event::ClientTimeout { peer },
        }
    }

    /// Updates connection state after handling a packet from the client.
    fn refresh_timeout(&mut self) {
        if let Some(timeout) = &mut self.timeout {
            timeout
                .as_mut()
                .reset(Instant::now() + CLIENT_TIMEOUT_INTERVAL)
        }
    }

    /// Updates connection state after sending a packet to the client.
    fn refresh_keepalive(&mut self) {
        match &mut self.variant {
            ConnectionVariant::Connected(connected) => connected.refresh_keepalive(),
            _ => unreachable!(),
        }
    }
}

impl ConnectedConnection {
    fn refresh_keepalive(&mut self) {
        self.keepalive
            .as_mut()
            .reset(Instant::now() + KEEPALIVE_INTERVAL);
    }
}

struct ClientIdAllocator {
    next: ClientId,
}

impl ClientIdAllocator {
    fn new() -> Self {
        Self {
            next: ClientId(NonZeroU8::new(1).unwrap()),
        }
    }

    fn allocate(&mut self) -> ClientId {
        let client_id = self.next;
        self.next = ClientId(NonZeroU8::new(self.next.0.get() + 1).unwrap());
        client_id
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fmt::{self, Debug, Display, Formatter};
    use std::marker::PhantomData;
    use std::time::Duration;

    use dungeon_vr_cryptography::{PrivateKey, PublicKey, SharedSecret};
    use dungeon_vr_shared::cancel;
    use dungeon_vr_shared::net_game::NetGame;
    use dungeon_vr_shared::protocol::challenge_token::ChallengeToken;
    use dungeon_vr_shared::protocol::connect_init_packet::ConnectInitPacket;
    use dungeon_vr_shared::protocol::packet::Packet;
    use dungeon_vr_shared::protocol::sealed::Sealed;
    use dungeon_vr_shared::protocol::{GAME_ID, SAFE_RECV_BUFFER_SIZE};
    use dungeon_vr_stream_codec::StreamCodec;
    use tokio::time::{interval, sleep, timeout_at, Instant};

    use crate::socket::testing::{FakeNetwork, FakeSocket};
    use crate::socket::BoundSocket;
    use crate::{
        ClientIdAllocator, Connection, ConnectionVariant, InnerServer, PendingConnection, TickId,
        CLIENT_TIMEOUT_INTERVAL, CONNECT_CHALLENGE_INTERVAL, TICK_INTERVAL,
    };

    use super::Server;

    async fn recv_packet<S: BoundSocket>(socket: &S) -> Packet {
        let mut buf = [0; SAFE_RECV_BUFFER_SIZE];
        let (size, _) = socket.recv_from(&mut buf).await.unwrap();
        let mut r = &buf[..size];
        let packet = Packet::read_from(&mut r).unwrap();
        assert!(r.is_empty());
        packet
    }

    async fn send_packet_to<S: BoundSocket>(
        socket: &S,
        packet: Packet,
        addr: <S as BoundSocket>::Addr,
    ) {
        let mut buf = Vec::new();
        packet.write_to(&mut buf).unwrap();
        socket.send_to(&buf, addr).await.unwrap();
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum FakeAddr {
        Server,
        Client1,
    }

    impl Display for FakeAddr {
        fn fmt(&self, f: &mut Formatter) -> fmt::Result {
            <Self as Debug>::fmt(self, f)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn handshake_integration_test() {
        let network = FakeNetwork::new();
        let server = Server::spawn(network.bind(FakeAddr::Server));
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

        server.shutdown().await.unwrap();
    }

    fn make_network_and_inner_server() -> (FakeNetwork<FakeAddr>, InnerServer<FakeSocket<FakeAddr>>)
    {
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

    struct InitWithPendingConnection {
        network: FakeNetwork<FakeAddr>,
        server: InnerServer<FakeSocket<FakeAddr>>,
        client_private_key: PrivateKey,
        client_public_key: PublicKey,
        server_private_key: PrivateKey,
        server_public_key: PublicKey,
        shared_secret: SharedSecret,
        token: ChallengeToken,
    }

    fn init_with_pending_connection() -> InitWithPendingConnection {
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
            client_public_key,
            server_private_key,
            server_public_key,
            shared_secret,
            token,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn recv_connectinit_when_no_connection_should_create_pending_connection() {
        let (network, mut server) = make_network_and_inner_server();
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

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        assert!(matches!(connection.variant, ConnectionVariant::Pending(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn recv_connectinit_when_connection_pending_should_not_change() {
        let InitWithPendingConnection {
            network,
            mut server,
            server_public_key,
            token,
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

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        let pending = match &connection.variant {
            ConnectionVariant::Pending(pending) => pending,
            _ => unreachable!(),
        };
        assert_eq!(server_public_key, pending.server_public_key);
        assert_eq!(token, pending.token);
    }

    #[tokio::test(start_paused = true)]
    async fn recv_connectresponse_when_connection_pending_should_connect() {
        let InitWithPendingConnection {
            network,
            mut server,
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

        server.run_once_for_test().await;

        let connection = server.connections.get(&FakeAddr::Client1).unwrap();
        assert!(matches!(
            connection.variant,
            ConnectionVariant::Connected(_),
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn pending_connection_should_send_challenges() {
        let InitWithPendingConnection {
            network,
            server,
            client_private_key,
            server_public_key,
            token,
            ..
        } = init_with_pending_connection();
        let socket = network.bind(FakeAddr::Client1);

        tokio::spawn(server.run());

        for _ in 0..3 {
            let packet = match recv_packet(&socket).await {
                Packet::ConnectChallenge(packet) => packet,
                _ => unreachable!(),
            };
            assert_eq!(server_public_key, packet.server_public_key);
            assert_eq!(
                token,
                packet
                    .sealed_payload
                    .open(
                        &client_private_key
                            .exchange(&packet.server_public_key)
                            .unwrap(),
                    )
                    .unwrap(),
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn pending_connection_should_time_out() {
        let InitWithPendingConnection { mut server, .. } = init_with_pending_connection();

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            timeout_at(deadline, server.run_once_for_test())
                .await
                .unwrap();
            let connection = server.connections.get(&FakeAddr::Client1).unwrap();
            if matches!(connection.variant, ConnectionVariant::Disconnecting(_)) {
                // Success.
                break;
            }
        }
    }
}
