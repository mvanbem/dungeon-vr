use std::convert::Infallible;
use std::future::pending;
use std::io;
use std::pin::Pin;

use dungeon_vr_cryptography::{KeyExchangeError, PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_protocol::challenge_token::ChallengeToken;
use dungeon_vr_protocol::connect_challenge_packet::ConnectChallengePacket;
use dungeon_vr_protocol::connect_init_packet::ConnectInitPacket;
use dungeon_vr_protocol::packet::Packet;
use dungeon_vr_protocol::sealed::Sealed;
use dungeon_vr_protocol::{GAME_ID, SAFE_RECV_BUFFER_SIZE};
use dungeon_vr_shared::cancel;
use dungeon_vr_socket::ConnectedSocket;
use dungeon_vr_stream_codec::{StreamCodec, UnframedByteVec};
use futures::FutureExt;
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, Duration, Instant, Interval, Sleep};

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

const CONNECTING_RESPONDING_SEND_INTERVAL: Duration = Duration::from_millis(250);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);
const SERVER_TIMEOUT_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_BUFFER_SIZE: usize = 256;
const EVENT_BUFFER_SIZE: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    SendGameData(Vec<u8>),
}

pub struct ClientConnection<S> {
    socket: S,
    requests: Option<mpsc::Receiver<Request>>,
    events: mpsc::Sender<Event>,
    recv_buffer: Pin<Box<[u8; SAFE_RECV_BUFFER_SIZE]>>,
    timeout: Option<Pin<Box<Sleep>>>,
    variant: Variant,
}

enum InternalEvent {
    Cancelled,
    Request(Option<Request>),
    SocketRecv(io::Result<usize>),
    ServerTimeout,
    SendIntervalElapsed,
    KeepaliveElapsed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    #[cfg(test)]
    Responding,
    Connected,
    Disconnected,
    GameData(Vec<u8>),
    Dropped,
}

enum ConfirmConnectionResult {
    Connected,
    Unchanged,
}

impl<S> ClientConnection<S>
where
    S: ConnectedSocket,
{
    pub fn spawn(socket: S) -> (cancel::Guard, mpsc::Sender<Request>, mpsc::Receiver<Event>)
    where
        S: ConnectedSocket + Send + Sync + 'static,
    {
        let cancel_token = cancel::Token::new();
        let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
        let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER_SIZE);

        let connection = Self::new(socket, request_rx, event_tx);
        tokio::spawn(connection.run(cancel_token.clone()));

        (cancel_token.guard(), request_tx, event_rx)
    }

    fn new(socket: S, requests: mpsc::Receiver<Request>, events: mpsc::Sender<Event>) -> Self {
        log::debug!("Connection state: connecting");
        let client_private_key = PrivateKey::gen();
        let client_public_key = client_private_key.to_public();
        let connect_state = Variant::Connecting {
            client_private_key,
            client_public_key,
            send_interval: interval(CONNECTING_RESPONDING_SEND_INTERVAL),
        };
        Self {
            socket,
            requests: Some(requests),
            events,
            recv_buffer: Box::pin([0; SAFE_RECV_BUFFER_SIZE]),
            timeout: Some(Box::pin(sleep(SERVER_TIMEOUT_INTERVAL))),
            variant: connect_state,
        }
    }

    async fn run(mut self, cancel_token: cancel::Token) {
        while !cancel_token.is_cancelled() {
            let requests = match &mut self.requests {
                Some(requests) => requests.recv().left_future(),
                None => pending().right_future(),
            };
            let timeout = match &mut self.timeout {
                Some(timeout) => timeout.left_future(),
                None => pending().right_future(),
            };

            let event = select! {
                biased;

                _ = cancel_token.cancelled() => InternalEvent::Cancelled,

                result = requests => InternalEvent::Request(result),

                result = self.socket.recv(&mut self.recv_buffer[..]) => InternalEvent::SocketRecv(result),

                _ = timeout => InternalEvent::ServerTimeout,

                event = self.variant.event() => event,
            };

            match event {
                InternalEvent::Cancelled => {
                    self.handle_cancelled().await;
                    break;
                }
                InternalEvent::Request(Some(request)) => self.handle_request(request).await,
                InternalEvent::Request(None) => self.requests = None,
                InternalEvent::SocketRecv(Ok(size)) => self.handle_socket_recv(size).await,
                InternalEvent::SocketRecv(Err(e)) => log::error!("Socket error: {e}"),
                InternalEvent::ServerTimeout => self.handle_server_timeout().await,
                InternalEvent::SendIntervalElapsed => self.handle_send_interval_elapsed().await,
                InternalEvent::KeepaliveElapsed => self.handle_keepalive_elapsed().await,
            }
        }

        // Drop everything except for the event sender. This lets the event receiver know the socket
        // has already been dropped when it receives the Dropped event.
        drop(self.socket);
        drop(self.requests);
        drop(self.recv_buffer);
        drop(self.timeout);
        drop(self.variant);
        drop(cancel_token);

        let _ = self.events.send(Event::Dropped).await;
    }

    async fn handle_cancelled(&mut self) {
        // TODO: Introduce a Disconnecting variant and send Disconnect packets for a while before
        // actually shutting down.
        match &self.variant {
            Variant::Disconnected => (),
            _ => {
                let _ = self.events.send(Event::Disconnected).await;
            }
        }
    }

    async fn handle_request(&mut self, request: Request) {
        match request {
            Request::SendGameData(data) => self.handle_send_game_data_request(data).await,
        }
    }

    async fn handle_send_game_data_request(&mut self, data: Vec<u8>) {
        let shared_secret = match &self.variant {
            Variant::Connected { shared_secret, .. } => shared_secret,
            _ => {
                log::debug!("Dropping outgoing game data: not connected");
                return;
            }
        };
        send_packet(
            &self.socket,
            Packet::GameData(Sealed::seal_ext::<UnframedByteVec>(data, shared_secret)),
        )
        .await;
    }

    async fn handle_socket_recv(&mut self, size: usize) {
        let mut r = &self.recv_buffer[..size];
        let packet = match Packet::read_from(&mut r) {
            Ok(packet) => packet,
            Err(e) => {
                log::debug!("Dropping invalid packet: {e}");
                return;
            }
        };
        if r.len() > 0 {
            log::debug!(
                "Dropping {:?} packet: {} unexpected trailing byte(s)",
                packet.kind(),
                r.len(),
            );
            return;
        }
        match packet {
            Packet::Disconnect(packet) => self.handle_disconnect_packet(packet).await,
            Packet::ConnectChallenge(packet) => self.handle_connect_challenge_packet(packet).await,
            Packet::Keepalive(packet) => self.handle_keepalive_packet(packet).await,
            Packet::GameData(packet) => self.handle_game_data_packet(packet).await,
            _ => {
                log::debug!("Dropping unsupported {:?} packet", packet.kind());
            }
        }
    }

    async fn handle_disconnect_packet(&mut self, packet: Sealed<()>) {
        let shared_secret = match self.variant.shared_secret() {
            Some(shared_secret) => shared_secret,
            None => {
                log::debug!("Dropping Disconnect packet: no shared secret");
                return;
            }
        };
        if let Err(e) = packet.open(shared_secret) {
            log::debug!("Dropping Disconnect packet: {e}");
            return;
        }
        log::info!("Connection state: disconnected");
        self.variant = Variant::Disconnected;
        let _ = self.events.send(Event::Disconnected).await;
    }

    async fn handle_connect_challenge_packet(&mut self, packet: ConnectChallengePacket) {
        let client_private_key = match &self.variant {
            Variant::Connecting {
                client_private_key, ..
            } => client_private_key,
            _ => {
                log::debug!("Dropping ConnectChallenge packet: wrong connection state");
                return;
            }
        };
        let shared_secret = match client_private_key.exchange(&packet.server_public_key) {
            Ok(shared_secret) => shared_secret,
            Err(KeyExchangeError::NonContributory) => {
                log::debug!("Dropping ConnectChallenge packet: non-contributory key exchange",);
                return;
            }
        };
        let token = match packet.sealed_payload.open(&shared_secret) {
            Ok(token) => token,
            Err(e) => {
                log::debug!("Dropping invalid ConnectChallenge packet: {e}");
                return;
            }
        };
        log::debug!("Connection state: responding to challenge");
        self.variant = Variant::Responding {
            shared_secret,
            token,
            send_interval: interval(CONNECTING_RESPONDING_SEND_INTERVAL),
        };
        #[cfg(test)]
        let _ = self.events.send(Event::Responding).await;
    }

    async fn handle_keepalive_packet(&mut self, packet: Sealed<()>) {
        let shared_secret = match self.variant.shared_secret() {
            Some(shared_secret) => shared_secret,
            None => {
                log::debug!("Dropping Keepalive packet: no shared secret");
                return;
            }
        };
        if let Err(e) = packet.open(shared_secret) {
            eprintln!("Dropping Keepalive packet: {e}");
            return;
        }
        self.refresh_timeout();
        if let ConfirmConnectionResult::Connected = self.confirm_connection() {
            let _ = self.events.send(Event::Connected).await;
        }
    }

    async fn handle_game_data_packet(&mut self, packet: Sealed<Vec<u8>>) {
        let shared_secret = match self.variant.shared_secret() {
            Some(shared_secret) => shared_secret,
            None => {
                log::debug!("Dropping GameData packet: no shared secret");
                return;
            }
        };
        let data = match packet.open_ext::<UnframedByteVec>(shared_secret) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("Dropping GameData packet: {e}");
                return;
            }
        };
        self.refresh_timeout();
        if let ConfirmConnectionResult::Connected = self.confirm_connection() {
            let _ = self.events.send(Event::Connected).await;
        }
        let _ = self.events.send(Event::GameData(data)).await;
    }

    async fn handle_server_timeout(&mut self) {
        self.timeout = None;
        self.variant = Variant::Disconnected;
        log::info!("Connection state: disconnected (timed out)");
        let _ = self.events.send(Event::Disconnected).await;
    }

    async fn handle_send_interval_elapsed(&mut self) {
        match &self.variant {
            Variant::Connecting {
                client_public_key, ..
            } => {
                send_packet(
                    &self.socket,
                    Packet::ConnectInit(ConnectInitPacket {
                        game_id: GAME_ID,
                        client_public_key: *client_public_key,
                    }),
                )
                .await;
            }
            Variant::Responding {
                shared_secret,
                token,
                ..
            } => {
                send_packet(
                    &self.socket,
                    Packet::ConnectResponse(Sealed::seal(*token, shared_secret)),
                )
                .await;
            }
            _ => unreachable!(),
        }
    }

    async fn handle_keepalive_elapsed(&mut self) {
        let shared_secret = self.variant.shared_secret().unwrap();
        send_packet(
            &self.socket,
            Packet::Keepalive(Sealed::seal((), shared_secret)),
        )
        .await;
        self.refresh_keepalive();
    }

    fn confirm_connection(&mut self) -> ConfirmConnectionResult {
        match self.variant {
            Variant::Responding { shared_secret, .. } => {
                log::info!("Connection state: connected");
                self.variant = Variant::Connected {
                    shared_secret,
                    keepalive: Box::pin(sleep(KEEPALIVE_INTERVAL)),
                };
                ConfirmConnectionResult::Connected
            }
            _ => ConfirmConnectionResult::Unchanged,
        }
    }

    /// Extends the timeout timer after handling a packet from the server.
    fn refresh_timeout(&mut self) {
        if let Some(timeout) = &mut self.timeout {
            timeout
                .as_mut()
                .reset(Instant::now() + SERVER_TIMEOUT_INTERVAL);
        }
    }

    /// Extends the keepalive timer after sending a packet to the server.
    fn refresh_keepalive(&mut self) {
        match &mut self.variant {
            Variant::Connected { keepalive, .. } => keepalive
                .as_mut()
                .reset(Instant::now() + KEEPALIVE_INTERVAL),
            _ => unreachable!(),
        }
    }
}

async fn send_packet<S, P>(socket: &S, packet: P)
where
    S: ConnectedSocket,
    P: StreamCodec<WriteError = Infallible>,
{
    let mut w = Vec::new();
    packet.write_to(&mut w).unwrap();
    if let Err(e) = socket.send(&w).await {
        log::error!("Unexpected socket error: {e}");
    }
}

enum Variant {
    /// Disconnected and idle.
    Disconnected,
    /// Send ConnectInit packets until receiving a ConnectChallenge.
    Connecting {
        client_private_key: PrivateKey,
        client_public_key: PublicKey,
        send_interval: Interval,
    },
    /// Send ConnectResponse packets until receiving a GameData or Keepalive packet.
    Responding {
        shared_secret: SharedSecret,
        token: ChallengeToken,
        send_interval: Interval,
    },
    /// Connection established. Exchanging GameData and Keepalive packets.
    Connected {
        shared_secret: SharedSecret,
        keepalive: Pin<Box<Sleep>>,
    },
}

impl Variant {
    fn shared_secret(&self) -> Option<&SharedSecret> {
        match self {
            Variant::Responding { shared_secret, .. }
            | Variant::Connected { shared_secret, .. } => Some(shared_secret),
            _ => None,
        }
    }

    async fn event(&mut self) -> InternalEvent {
        let (send_interval, keepalive) = match self {
            Self::Disconnected { .. } => (pending().right_future(), pending().right_future()),
            Self::Connecting { send_interval, .. } => {
                (send_interval.tick().left_future(), pending().right_future())
            }
            Self::Responding { send_interval, .. } => {
                (send_interval.tick().left_future(), pending().right_future())
            }
            Self::Connected { keepalive, .. } => {
                (pending().right_future(), keepalive.as_mut().left_future())
            }
        };

        select! {
            biased;

            _ = send_interval => InternalEvent::SendIntervalElapsed,

            _ = keepalive => InternalEvent::KeepaliveElapsed,
        }
    }
}
