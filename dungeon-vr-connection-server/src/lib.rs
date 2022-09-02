use std::collections::HashMap;
use std::future::pending;
use std::io;
use std::marker::PhantomData;
use std::pin::Pin;

use dungeon_vr_connection_shared::challenge_token::ChallengeToken;
use dungeon_vr_connection_shared::connect_challenge_packet::ConnectChallengePacket;
use dungeon_vr_connection_shared::connect_init_packet::ConnectInitPacket;
use dungeon_vr_connection_shared::packet::Packet;
use dungeon_vr_connection_shared::sealed::Sealed;
use dungeon_vr_connection_shared::{GAME_ID, SAFE_RECV_BUFFER_SIZE};
use dungeon_vr_cryptography::{KeyExchangeError, PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_socket::{AddrBound, BoundSocket};
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
pub enum Request<Addr> {
    SendGameData { addr: Addr, data: Vec<u8> },
}

pub struct ConnectionServer<Addr> {
    socket: Box<dyn BoundSocket<Addr>>,
    requests: Option<mpsc::Receiver<Request<Addr>>>,
    events: mpsc::Sender<Event<Addr>>,
    recv_buffer: Pin<Box<[u8; SAFE_RECV_BUFFER_SIZE]>>,
    connections: HashMap<Addr, Connection<Addr>>,
}

#[derive(Debug)]
enum InternalEvent<Addr> {
    Cancelled,
    Request(Option<Request<Addr>>),
    SocketRecv(io::Result<(usize, Addr)>),
    ClientTimeout { addr: Addr },
    DisconnectElapsed { addr: Addr },
    SendIntervalElapsed { addr: Addr },
    KeepaliveElapsed { addr: Addr },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Pending,
    Connected,
    Disconnecting,
}

#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event<Addr> {
    State { addr: Addr, state: ConnectionState },
    GameData { addr: Addr, data: Vec<u8> },
    Dropped,
}

impl<Addr: AddrBound> ConnectionServer<Addr> {
    pub fn spawn(
        socket: Box<dyn BoundSocket<Addr>>,
    ) -> (
        cancel::Guard,
        mpsc::Sender<Request<Addr>>,
        mpsc::Receiver<Event<Addr>>,
    ) {
        let cancel_token = cancel::Token::new();
        let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
        let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER_SIZE);

        let connection = Self::new(socket, request_rx, event_tx);
        tokio::spawn(connection.run(cancel_token.clone()));

        (cancel_token.guard(), request_tx, event_rx)
    }

    fn new(
        socket: Box<dyn BoundSocket<Addr>>,
        requests: mpsc::Receiver<Request<Addr>>,
        events: mpsc::Sender<Event<Addr>>,
    ) -> Self {
        Self {
            socket,
            requests: Some(requests),
            events,
            recv_buffer: Box::pin([0; SAFE_RECV_BUFFER_SIZE]),
            connections: HashMap::new(),
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
                .map(|(addr, connection)| connection.wait_for_event(*addr))
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
                InternalEvent::SocketRecv(Ok((size, addr))) => {
                    self.handle_socket_recv(size, addr).await
                }
                InternalEvent::SocketRecv(Err(_)) => {
                    // NOTE: Ignore any error. Windows in particular returns errors based on
                    // previous ICMP activity. These are not useful. If connectivity is disrupted,
                    // the timeout mechanism will eventually notice.
                }
                InternalEvent::ClientTimeout { addr } => self.handle_client_timeout(addr).await,
                InternalEvent::DisconnectElapsed { addr } => {
                    self.handle_disconnect_elapsed(addr).await
                }
                InternalEvent::SendIntervalElapsed { addr } => {
                    self.handle_pending_connection_send(addr).await
                }
                InternalEvent::KeepaliveElapsed { addr } => {
                    self.handle_keepalive_elapsed(addr).await
                }
            }
        }

        // Drop everything except for the event sender. This lets the event receiver know the socket
        // has already been dropped when it receives the Dropped event.
        drop(self.socket);
        drop(self.requests);
        drop(self.recv_buffer);
        drop(self.connections);
        drop(cancel_token);

        let _ = self.events.send(Event::Dropped).await;
    }

    async fn handle_cancelled(&mut self) {
        // TODO: Introduce a Disconnecting variant and put all connections into the Disconnecting
        // state until they finish shutting down.

        // For now, just tell the event reciever that each connected player has disconnected.

        for (addr, _) in self.connections.drain() {
            let _ = self
                .events
                .send(Event::State {
                    addr,
                    state: ConnectionState::Disconnected,
                })
                .await;
        }
    }

    async fn handle_request(&mut self, request: Request<Addr>) {
        match request {
            Request::SendGameData { addr, data } => {
                self.handle_send_game_data_request(addr, data).await
            }
        }
    }

    async fn handle_send_game_data_request(&mut self, addr: Addr, data: Vec<u8>) {
        match self.connections.get(&addr) {
            Some(connection) => {
                let socket = &*self.socket;
                send_packet(
                    socket,
                    addr,
                    Packet::GameData(Sealed::seal_ext::<UnframedByteVec>(
                        data,
                        &connection.shared_secret,
                    )),
                )
                .await;
            }
            None => {
                log::debug!("Dropping outgoing game data: no connection for addr {addr}",);
            }
        }
    }

    async fn handle_socket_recv(&mut self, size: usize, addr: Addr) {
        let mut r = &self.recv_buffer[..size];
        let packet = match Packet::read_from(&mut r) {
            Ok(packet) => packet,
            Err(e) => {
                log::debug!("Client {addr}: Dropping invalid packet: {e}");
                return;
            }
        };
        if !r.is_empty() {
            log::debug!(
                "Client {addr}: Dropping {:?} packet: {} unexpected trailing byte(s)",
                packet.kind(),
                r.len(),
            );
            return;
        }
        match packet {
            Packet::Disconnect(sealed) => self.handle_disconnect_packet(addr, sealed).await,
            Packet::ConnectInit(packet) => self.handle_connect_init_packet(addr, packet).await,
            Packet::ConnectResponse(sealed) => {
                self.handle_connect_response_packet(addr, sealed).await;
            }
            Packet::Keepalive(sealed) => self.handle_keepalive_packet(addr, sealed),
            Packet::GameData(sealed) => self.handle_game_data_packet(addr, sealed).await,
            _ => {
                log::debug!(
                    "Client {addr}: Dropping unexpected {:?} packet",
                    packet.kind(),
                );
            }
        }
    }

    async fn handle_disconnect_packet(&mut self, addr: Addr, sealed: Sealed<()>) {
        let connection = match self.connections.get_mut(&addr) {
            Some(connection) => connection,
            None => {
                log::debug!("Client {addr}: Dropping Disconnect packet: not connected");
                return;
            }
        };
        if let Err(e) = sealed.open(&connection.shared_secret) {
            log::debug!("Client {addr}: Dropping Disconnect packet: {e}");
            return;
        }

        let event = match connection.variant {
            // TODO: Think about this a bit more.
            ConnectionVariant::Pending(_)
            | ConnectionVariant::Connected(ConnectedConnection { .. }) => Some(Event::State {
                addr,
                state: ConnectionState::Disconnected,
            }),
            _ => None,
        };
        self.connections.remove(&addr);
        log::info!("Client {addr}: Disconnected");
        if let Some(event) = event {
            let _ = self.events.send(event).await;
        }
    }

    async fn handle_connect_init_packet(&mut self, addr: Addr, packet: ConnectInitPacket) {
        if packet.game_id != GAME_ID {
            log::debug!(
                "Client {addr}: Dropping ConnectInit paket: unsupported game ID 0x{:08x}",
                packet.game_id,
            );
            return;
        }
        if self.connections.contains_key(&addr) {
            log::debug!("Client {addr}: Dropping redundant ConnectInit packet");
            return;
        }

        // Perform our side of the ECDH key exchange.
        let private_key = PrivateKey::gen();
        let server_public_key = private_key.to_public();
        let shared_secret = match private_key.exchange(&packet.client_public_key) {
            Ok(shared_secret) => shared_secret,
            Err(KeyExchangeError::NonContributory) => {
                log::debug!(
                    "Client {addr}: Dropping ConnectInit packet: non-contributory key exchange"
                );
                return;
            }
        };

        // Record the new connection.
        let token = ChallengeToken::gen();
        self.connections.insert(
            addr,
            Connection {
                shared_secret,
                timeout: Some(Box::pin(sleep(CLIENT_TIMEOUT_INTERVAL))),
                variant: ConnectionVariant::Pending(PendingConnection {
                    server_public_key,
                    token,
                    send_interval: interval(SEND_INTERVAL),
                }),
                _phantom_addr: PhantomData,
            },
        );
        log::info!("Client {addr}: New connection pending");
        let _ = self
            .events
            .send(Event::State {
                addr,
                state: ConnectionState::Pending,
            })
            .await;
    }

    async fn handle_connect_response_packet(&mut self, addr: Addr, sealed: Sealed<ChallengeToken>) {
        let connection = match self.connections.get_mut(&addr) {
            Some(connection) => connection,
            None => {
                log::debug!("Client {addr}: Dropping ConnectResponse packet: not connected");
                return;
            }
        };
        let packet_token = match sealed.open(&connection.shared_secret) {
            Ok(token) => token,
            Err(e) => {
                log::debug!("Client {addr}: Dropping ConnectResponse packet: {e}");
                return;
            }
        };
        match connection.variant {
            ConnectionVariant::Pending(PendingConnection { token, .. }) => {
                if packet_token != token {
                    log::debug!(
                        "Client {addr}: Dropping ConnectResponse packet: bad challenge token",
                    );
                    return;
                }
                // OK to proceed.
            }
            ConnectionVariant::Connected(_) => {
                log::debug!("Client {addr}: Dropping redundant ConnectResponse packet");
                return;
            }
            ConnectionVariant::Disconnecting(_) => {
                log::debug!("Client {addr}: Dropping ConnectResponse packet: disconnecting");
                return;
            }
        }

        // Advance this connection to the Connected state.
        connection.variant = ConnectionVariant::Connected(ConnectedConnection {
            keepalive: Box::pin(sleep(Duration::ZERO)),
        });
        log::info!("Client {addr}: Connected");
        let _ = self
            .events
            .send(Event::State {
                addr,
                state: ConnectionState::Connected,
            })
            .await;
    }

    fn handle_keepalive_packet(&mut self, addr: Addr, sealed: Sealed<()>) {
        let connection = match self.connections.get_mut(&addr) {
            Some(connection) => connection,
            None => {
                log::debug!("Client {addr}: Dropping Keepalive packet: not connected");
                return;
            }
        };
        match sealed.open(&connection.shared_secret) {
            Ok(()) => (),
            Err(e) => {
                log::debug!("Client {addr}: Dropping Keepalive packet: {e}");
                return;
            }
        }
        connection.refresh_timeout();
    }

    async fn handle_game_data_packet(&mut self, addr: Addr, sealed: Sealed<Vec<u8>>) {
        let connection = match self.connections.get_mut(&addr) {
            Some(connection) => connection,
            None => {
                log::debug!("Client {addr}: Dropping GameData packet: not connected");
                return;
            }
        };
        match connection.variant {
            ConnectionVariant::Connected(ConnectedConnection { .. }) => (),
            _ => {
                log::debug!("Client {addr}: Dropping GameData packet: not connected");
                return;
            }
        }
        let data = match sealed.open_ext::<UnframedByteVec>(&connection.shared_secret) {
            Ok(game_data) => game_data,
            Err(e) => {
                log::debug!("Client {addr}: Dropping GameData packet: {e}");
                return;
            }
        };
        connection.refresh_timeout();
        let _ = self.events.send(Event::GameData { addr, data }).await;
    }

    async fn handle_client_timeout(&mut self, addr: Addr) {
        let connection = self.connections.get_mut(&addr).unwrap();
        let event = match connection.variant {
            ConnectionVariant::Pending(_)
            | ConnectionVariant::Connected(ConnectedConnection { .. }) => Some(Event::State {
                addr,
                state: ConnectionState::Disconnecting,
            }),
            _ => None,
        };
        connection.timeout = None;
        connection.variant = ConnectionVariant::Disconnecting(DisconnectingConnection {
            interval: interval(SEND_INTERVAL),
            packets_to_send: DISCONNECT_PACKET_COUNT,
        });
        log::info!("Client {addr}: Disconnecting (timed out)");
        if let Some(event) = event {
            let _ = self.events.send(event).await;
        }
    }

    async fn handle_disconnect_elapsed(&mut self, addr: Addr) {
        let connection = self.connections.get_mut(&addr).unwrap();
        let disconnecting = match &mut connection.variant {
            ConnectionVariant::Disconnecting(disconnecting) => disconnecting,
            _ => unreachable!(),
        };

        let socket = &*self.socket;
        send_packet(
            socket,
            addr,
            Packet::Disconnect(Sealed::seal((), &connection.shared_secret)),
        )
        .await;

        disconnecting.packets_to_send -= 1;
        if disconnecting.packets_to_send == 0 {
            self.connections.remove(&addr);
            log::debug!("Client {addr}: Done sending Disconnect packets")
        }
    }

    async fn handle_pending_connection_send(&mut self, addr: Addr) {
        let connection = self.connections.get_mut(&addr).unwrap();
        let pending = match &connection.variant {
            ConnectionVariant::Pending(pending) => pending,
            _ => unreachable!(),
        };
        let socket = &*self.socket;
        send_packet(
            socket,
            addr,
            Packet::ConnectChallenge(ConnectChallengePacket {
                server_public_key: pending.server_public_key,
                sealed_payload: Sealed::seal(pending.token, &connection.shared_secret),
            }),
        )
        .await;
    }

    async fn handle_keepalive_elapsed(&mut self, addr: Addr) {
        let connection = self.connections.get_mut(&addr).unwrap();
        let socket = &*self.socket;
        send_packet(
            socket,
            addr,
            Packet::Keepalive(Sealed::seal((), &connection.shared_secret)),
        )
        .await;
        connection.refresh_keepalive();
    }
}

async fn send_packet<Addr: AddrBound>(socket: &dyn BoundSocket<Addr>, addr: Addr, packet: Packet) {
    let mut w = Vec::new();
    packet.write_to(&mut w).unwrap();
    // NOTE: Ignore any error. Windows in particular returns errors based on previous ICMP activity.
    // These are not useful. If connectivity is disrupted, the timeout mechanism will eventually
    // notice.
    _ = socket.send_to(&w, addr).await;
}

struct Connection<Addr> {
    shared_secret: SharedSecret,
    timeout: Option<Pin<Box<Sleep>>>,
    variant: ConnectionVariant,
    _phantom_addr: PhantomData<Addr>,
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
    keepalive: Pin<Box<Sleep>>,
}

struct DisconnectingConnection {
    interval: Interval,
    packets_to_send: usize,
}

impl<Addr> Connection<Addr> {
    async fn wait_for_event(&mut self, addr: Addr) -> InternalEvent<Addr> {
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

            _ = send_interval => InternalEvent::SendIntervalElapsed { addr },

            _ = keepalive_elapsed => InternalEvent::KeepaliveElapsed { addr },

            _ = disconnect_elapsed => InternalEvent::DisconnectElapsed { addr },

            _ = timeout => InternalEvent::ClientTimeout { addr },
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
