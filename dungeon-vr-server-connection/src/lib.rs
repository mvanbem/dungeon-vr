use std::collections::HashMap;
use std::convert::Infallible;
use std::future::pending;
use std::io;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::pin::Pin;

use dungeon_vr_cryptography::{KeyExchangeError, PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_protocol::challenge_token::ChallengeToken;
use dungeon_vr_protocol::connect_challenge_packet::ConnectChallengePacket;
use dungeon_vr_protocol::connect_init_packet::ConnectInitPacket;
use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_protocol::sealed::Sealed;
use dungeon_vr_protocol::{GAME_ID, SAFE_RECV_BUFFER_SIZE};
use dungeon_vr_shared::cancel;
use dungeon_vr_shared::net_game::PlayerId;
use dungeon_vr_socket::BoundSocket;
use dungeon_vr_stream_codec::{StreamCodec, UnframedByteVec};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, Duration, Instant, Interval, Sleep};

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

const SEND_INTERVAL: Duration = Duration::from_millis(250);
const DISCONNECT_PACKET_COUNT: usize = 10;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);
const CLIENT_TIMEOUT_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_BUFFER_SIZE: usize = 256;
const EVENT_BUFFER_SIZE: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    SendGameData { player_id: PlayerId, data: Vec<u8> },
}

pub struct ServerConnection<S: BoundSocket> {
    socket: S,
    requests: Option<mpsc::Receiver<Request>>,
    events: mpsc::Sender<Event>,
    recv_buffer: Pin<Box<[u8; SAFE_RECV_BUFFER_SIZE]>>,
    connections: HashMap<<S as BoundSocket>::Addr, Connection<S>>,
    player_ids: PlayerIdAllocator,
}

#[derive(Debug)]
enum InternalEvent<A> {
    Cancelled,
    Request(Option<Request>),
    SocketRecv(io::Result<(usize, A)>),
    ClientTimeout { peer: A },
    DisconnectElapsed { peer: A },
    SendIntervalElapsed { peer: A },
    KeepaliveElapsed { peer: A },
}

#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    #[cfg(test)]
    PeerConnecting,
    #[cfg(test)]
    PeerDisconnected,
    PlayerConnected {
        player_id: PlayerId,
    },
    PlayerDisconnected {
        player_id: PlayerId,
    },
    GameData {
        player_id: PlayerId,
        data: Vec<u8>,
    },
    Dropped,
}

impl<S: BoundSocket> ServerConnection<S> {
    pub fn spawn(socket: S) -> (cancel::Guard, mpsc::Sender<Request>, mpsc::Receiver<Event>)
    where
        S: BoundSocket + Send + Sync + 'static,
    {
        let cancel_token = cancel::Token::new();
        let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
        let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER_SIZE);

        let connection = Self::new(socket, request_rx, event_tx);
        tokio::spawn(connection.run(cancel_token.clone()));

        (cancel_token.guard(), request_tx, event_rx)
    }

    fn new(socket: S, requests: mpsc::Receiver<Request>, events: mpsc::Sender<Event>) -> Self {
        Self {
            socket,
            requests: Some(requests),
            events,
            recv_buffer: Box::pin([0; SAFE_RECV_BUFFER_SIZE]),
            connections: HashMap::new(),
            player_ids: PlayerIdAllocator::new(),
        }
    }

    async fn run(mut self, cancel_token: cancel::Token) {
        while !cancel_token.is_cancelled() {
            let requests = match &mut self.requests {
                Some(requests) => requests.recv().left_future(),
                None => pending().right_future(),
            };
            let mut dynamic_events: FuturesUnordered<_> = self
                .connections
                .iter_mut()
                .map(|(peer, connection)| connection.wait_for_event(*peer))
                .collect();

            let event = select! {
                biased;

                _ = cancel_token.cancelled() => InternalEvent::Cancelled,

                result = requests => InternalEvent::Request(result),

                result = self.socket.recv_from(&mut self.recv_buffer[..]) => InternalEvent::SocketRecv(result),

                Some(event) = dynamic_events.next() => event,
            };
            drop(dynamic_events);

            match event {
                InternalEvent::Cancelled => {
                    self.handle_cancelled().await;
                    break;
                }
                InternalEvent::Request(Some(request)) => self.handle_request(request).await,
                InternalEvent::Request(None) => self.requests = None,
                InternalEvent::SocketRecv(Ok((size, peer))) => {
                    self.handle_socket_recv(size, peer).await
                }
                InternalEvent::SocketRecv(Err(e)) => log::error!("Unexpected socket error: {e}"),
                InternalEvent::ClientTimeout { peer } => self.handle_client_timeout(peer).await,
                InternalEvent::DisconnectElapsed { peer } => {
                    self.handle_disconnect_elapsed(peer).await
                }
                InternalEvent::SendIntervalElapsed { peer } => {
                    self.handle_pending_connection_send(peer).await
                }
                InternalEvent::KeepaliveElapsed { peer } => {
                    self.handle_keepalive_elapsed(peer).await
                }
            }
        }

        // Drop everything except for the event sender. This lets the event receiver know the socket
        // has already been dropped when it receives the Dropped event.
        drop(self.socket);
        drop(self.requests);
        drop(self.recv_buffer);
        drop(self.connections);
        drop(self.player_ids);
        drop(cancel_token);

        let _ = self.events.send(Event::Dropped).await;
    }

    async fn handle_cancelled(&mut self) {
        // TODO: Introduce a Disconnecting variant and put all connections into the Disconnecting
        // state until they finish shutting down.

        // For now, just tell the event reciever that each connected player has disconnected.

        for (_peer, connection) in self.connections.drain() {
            if let ConnectionVariant::Connected(connected) = connection.variant {
                let _ = self
                    .events
                    .send(Event::PlayerDisconnected {
                        player_id: connected.player_id,
                    })
                    .await;
            };
        }
    }

    async fn handle_request(&mut self, request: Request) {
        match request {
            Request::SendGameData { player_id, data } => {
                self.handle_send_game_data_request(player_id, data).await
            }
        }
    }

    async fn handle_send_game_data_request(&mut self, player_id: PlayerId, data: Vec<u8>) {
        // TODO: Secondary index of connected connections by player ID!
        for (&peer, connection) in &self.connections {
            if let ConnectionVariant::Connected(connected) = &connection.variant {
                if connected.player_id == player_id {
                    send_packet(
                        &self.socket,
                        peer,
                        Packet::GameData(Sealed::seal_ext::<UnframedByteVec>(
                            data,
                            &connection.shared_secret,
                        )),
                    )
                    .await;
                    return;
                }
            }
        }
        log::debug!("Dropping outgoing game data: no connection for player ID {player_id:?}",);
    }

    async fn handle_socket_recv(&mut self, size: usize, peer: <S as BoundSocket>::Addr) {
        let mut r = &self.recv_buffer[..size];
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
            Packet::Disconnect(sealed) => self.handle_disconnect_packet(peer, sealed).await,
            Packet::ConnectInit(packet) => self.handle_connect_init_packet(peer, packet).await,
            Packet::ConnectResponse(sealed) => {
                self.handle_connect_response_packet(peer, sealed).await;
            }
            Packet::Keepalive(sealed) => self.handle_keepalive_packet(peer, sealed),
            Packet::GameData(sealed) => self.handle_game_data_packet(peer, sealed).await,
            _ => {
                log::debug!(
                    "Peer {peer}: Dropping unexpected {:?} packet",
                    packet.kind(),
                );
            }
        }
    }

    async fn handle_disconnect_packet(
        &mut self,
        peer: <S as BoundSocket>::Addr,
        sealed: Sealed<()>,
    ) {
        let connection = match self.connections.get_mut(&peer) {
            Some(connection) => connection,
            None => {
                log::debug!("Peer {peer}: Dropping Disconnect packet: not connected");
                return;
            }
        };
        if let Err(e) = sealed.open(&connection.shared_secret) {
            log::debug!("Peer {peer}: Dropping Disconnect packet: {e}");
            return;
        }

        let event = match connection.variant {
            #[cfg(test)]
            ConnectionVariant::Pending(_) => Some(Event::PeerDisconnected),
            ConnectionVariant::Connected(ConnectedConnection { player_id, .. }) => {
                Some(Event::PlayerDisconnected { player_id })
            }
            _ => None,
        };
        self.connections.remove(&peer);
        log::info!("Peer {peer}: Disconnected");
        if let Some(event) = event {
            let _ = self.events.send(event).await;
        }
    }

    async fn handle_connect_init_packet(
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
                    send_interval: interval(SEND_INTERVAL),
                }),
                _phantom_socket: PhantomData,
            },
        );
        log::info!("Peer {peer}: New connection pending");
        #[cfg(test)]
        let _ = self.events.send(Event::PeerConnecting).await;
    }

    async fn handle_connect_response_packet(
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
        let player_id = self.player_ids.allocate();
        connection.variant = ConnectionVariant::Connected(ConnectedConnection {
            player_id,
            keepalive: Box::pin(sleep(Duration::ZERO)),
        });
        log::info!("Peer {peer}: Connected");
        let _ = self.events.send(Event::PlayerConnected { player_id }).await;
    }

    fn handle_keepalive_packet(&mut self, peer: <S as BoundSocket>::Addr, sealed: Sealed<()>) {
        let connection = match self.connections.get_mut(&peer) {
            Some(connection) => connection,
            None => {
                log::debug!("Peer {peer}: Dropping Keepalive packet: not connected");
                return;
            }
        };
        match sealed.open(&connection.shared_secret) {
            Ok(()) => (),
            Err(e) => {
                log::debug!("Peer {peer}: Dropping Keepalive packet: {e}");
                return;
            }
        }
        connection.refresh_timeout();
    }

    async fn handle_game_data_packet(
        &mut self,
        peer: <S as BoundSocket>::Addr,
        sealed: Sealed<Vec<u8>>,
    ) {
        let connection = match self.connections.get_mut(&peer) {
            Some(connection) => connection,
            None => {
                log::debug!("Peer {peer}: Dropping GameData packet: not connected");
                return;
            }
        };
        let player_id = match connection.variant {
            ConnectionVariant::Connected(ConnectedConnection { player_id, .. }) => player_id,
            _ => {
                log::debug!("Peer {peer}: Dropping GameData packet: not connected");
                return;
            }
        };
        let data = match sealed.open_ext::<UnframedByteVec>(&connection.shared_secret) {
            Ok(game_data) => game_data,
            Err(e) => {
                log::debug!("Peer {peer}: Dropping GameData packet: {e}");
                return;
            }
        };
        connection.refresh_timeout();
        let _ = self.events.send(Event::GameData { player_id, data }).await;
    }

    async fn handle_client_timeout(&mut self, peer: <S as BoundSocket>::Addr) {
        let connection = self.connections.get_mut(&peer).unwrap();
        let event = match connection.variant {
            #[cfg(test)]
            ConnectionVariant::Pending(_) => Some(Event::PeerDisconnected),
            ConnectionVariant::Connected(ConnectedConnection { player_id, .. }) => {
                Some(Event::PlayerDisconnected { player_id })
            }
            _ => None,
        };
        connection.timeout = None;
        connection.variant = ConnectionVariant::Disconnecting(DisconnectingConnection {
            interval: interval(SEND_INTERVAL),
            packets_to_send: DISCONNECT_PACKET_COUNT,
        });
        log::info!("Peer {peer}: Disconnecting (timed out)");
        if let Some(event) = event {
            let _ = self.events.send(event).await;
        }
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
    player_id: PlayerId,
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
    ) -> InternalEvent<<S as BoundSocket>::Addr> {
        let timeout = match &mut self.timeout {
            Some(timeout) => timeout.left_future(),
            None => pending().right_future(),
        };
        let (send_interval, keepalive_elapsed, disconnect_elapsed) = match &mut self.variant {
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

            _ = send_interval => InternalEvent::SendIntervalElapsed { peer },

            _ = keepalive_elapsed => InternalEvent::KeepaliveElapsed { peer },

            _ = disconnect_elapsed => InternalEvent::DisconnectElapsed { peer },

            _ = timeout => InternalEvent::ClientTimeout { peer },
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

struct PlayerIdAllocator {
    next: PlayerId,
}

impl PlayerIdAllocator {
    fn new() -> Self {
        Self {
            next: PlayerId(NonZeroU8::new(1).unwrap()),
        }
    }

    fn allocate(&mut self) -> PlayerId {
        let player_id = self.next;
        self.next = PlayerId(NonZeroU8::new(self.next.0.get() + 1).unwrap());
        player_id
    }
}
