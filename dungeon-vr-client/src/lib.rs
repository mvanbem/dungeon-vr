use std::convert::Infallible;
use std::future::pending;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::pin::Pin;

use dungeon_vr_cryptography::{KeyExchangeError, PrivateKey, PublicKey, SharedSecret};
use dungeon_vr_shared::cancel;
use dungeon_vr_shared::protocol::challenge_token::ChallengeToken;
use dungeon_vr_shared::protocol::connect_challenge_packet::ConnectChallengePacket;
use dungeon_vr_shared::protocol::connect_init_packet::ConnectInitPacket;
use dungeon_vr_shared::protocol::packet::Packet;
use dungeon_vr_shared::protocol::sealed::Sealed;
use dungeon_vr_shared::protocol::{GAME_ID, SAFE_RECV_BUFFER_SIZE};
use dungeon_vr_stream_codec::StreamCodec;
use tokio::net::UdpSocket;
use tokio::select;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::{interval, sleep, Duration, Instant, Interval, Sleep};

const SEND_INTERVAL: Duration = Duration::from_millis(250);
const SERVER_TIMEOUT_INTERVAL: Duration = Duration::from_secs(5);

pub struct Client {
    cancel_guard: cancel::Guard,
    join_handle: JoinHandle<()>,
}

impl Client {
    pub async fn spawn(server_addr: SocketAddr) -> io::Result<Self> {
        let cancel = cancel::Token::new();
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).await?;
        socket.connect(server_addr).await?;
        let join_handle = tokio::spawn(InnerClient::new(socket).run(cancel.clone()));
        Ok(Client {
            cancel_guard: cancel.guard(),
            join_handle,
        })
    }

    pub async fn shutdown(self) -> Result<(), JoinError> {
        drop(self.cancel_guard);
        self.join_handle.await?;
        Ok(())
    }
}

struct InnerClient {
    socket: UdpSocket,
    connect_state: ConnectionState,
}

enum Event {
    SocketRecv(io::Result<usize>),
    ConnectStateIntervalElapsed,
}

impl InnerClient {
    fn new(socket: UdpSocket) -> Self {
        log::debug!("Connection state: initializing");
        let client_private_key = PrivateKey::gen();
        let client_public_key = client_private_key.to_public();
        let connect_state = ConnectionState::Init {
            client_private_key,
            client_public_key,
            interval: interval(SEND_INTERVAL),
        };
        Self {
            socket,
            connect_state,
        }
    }

    async fn run(mut self, cancel: cancel::Token) {
        let mut buf = [0; SAFE_RECV_BUFFER_SIZE];
        while !cancel.is_cancelled() {
            let event = select! {
                biased;

                _ = cancel.cancelled() => break,

                result = self.socket.recv(&mut buf[..]) => Event::SocketRecv(result),

                event = self.connect_state.run() => event,
            };
            match event {
                Event::SocketRecv(Ok(size)) => self.handle_socket_recv(&buf[..size]),
                Event::SocketRecv(Err(e)) => log::error!("Socket error: {e}"),
                Event::ConnectStateIntervalElapsed => {
                    self.handle_connect_state_interval_elapsed().await
                }
            }
        }
    }

    fn handle_socket_recv(&mut self, mut r: &[u8]) {
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
            Packet::Disconnect(packet) => self.handle_disconnect_packet(packet),
            Packet::ConnectChallenge(packet) => self.handle_connect_challenge_packet(packet),
            Packet::Keepalive(packet) => self.handle_keepalive_packet(packet),
            _ => {
                log::debug!("Dropping unsupported {:?} packet", packet.kind());
            }
        }
    }

    fn handle_disconnect_packet(&mut self, packet: Sealed<()>) {
        let shared_secret = match self.connect_state.shared_secret() {
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
        self.connect_state = ConnectionState::Disconnected;
    }

    fn handle_connect_challenge_packet(&mut self, packet: ConnectChallengePacket) {
        match &self.connect_state {
            ConnectionState::Init {
                client_private_key, ..
            } => {
                let shared_secret = match client_private_key.exchange(&packet.server_public_key) {
                    Ok(shared_secret) => shared_secret,
                    Err(KeyExchangeError::NonContributory) => {
                        log::debug!(
                            "Dropping ConnectChallenge packet: non-contributory key exchange",
                        );
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
                self.connect_state = ConnectionState::Response {
                    shared_secret,
                    token,
                    interval: interval(SEND_INTERVAL),
                };
            }
            _ => log::debug!("Dropping ConnectChallenge packet: wrong connection state"),
        }
    }

    fn handle_keepalive_packet(&mut self, packet: Sealed<()>) {
        let shared_secret = match self.connect_state.shared_secret() {
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
        self.connect_state.refresh_timeout();

        // The keepalive packet doesn't do anything beyond touching the connection.
    }

    async fn handle_connect_state_interval_elapsed(&mut self) {
        match &self.connect_state {
            ConnectionState::Init {
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
            ConnectionState::Response {
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
}

async fn send_packet<P>(socket: &UdpSocket, packet: P)
where
    P: StreamCodec<WriteError = Infallible>,
{
    let mut w = Vec::new();
    packet.write_to(&mut w).unwrap();
    if let Err(e) = socket.send(&w).await {
        log::error!("Unexpected socket error: {e}");
    }
}
enum ConnectionState {
    /// Disconnected and idle.
    Disconnected,
    /// Send ConnectInit packets until receiving a ConnectChallenge.
    Init {
        client_private_key: PrivateKey,
        client_public_key: PublicKey,
        interval: Interval,
    },
    /// Send ConnectResponse packets until receiving any session packet.
    Response {
        shared_secret: SharedSecret,
        token: ChallengeToken,
        interval: Interval,
    },
    /// Connection established. Exchanging session packets normally.
    Connected {
        shared_secret: SharedSecret,
        timeout: Pin<Box<Sleep>>,
    },
}

impl ConnectionState {
    fn shared_secret(&self) -> Option<&SharedSecret> {
        match self {
            ConnectionState::Response { shared_secret, .. }
            | ConnectionState::Connected { shared_secret, .. } => Some(shared_secret),
            _ => None,
        }
    }

    async fn run(&mut self) -> Event {
        match self {
            Self::Disconnected { .. } => pending().await,
            Self::Init { interval, .. } => drop(interval.tick().await),
            Self::Response { interval, .. } => drop(interval.tick().await),
            Self::Connected { .. } => pending().await,
        }
        Event::ConnectStateIntervalElapsed
    }

    /// Updates connection state after handling a packet from the server.
    ///
    /// This method transitions from the Response state to the Connected state because there is no
    /// special acknowledgement for a successful challenge-response sequence; receiving any non-
    /// challenge-response packet from the server implies having succeeded. Otherwise, in the
    /// Connected state, this method resets the timeout alarm.
    fn refresh_timeout(&mut self) {
        match self {
            Self::Response { shared_secret, .. } => {
                log::info!("Connection state: connected");
                *self = ConnectionState::Connected {
                    shared_secret: shared_secret.clone(),
                    timeout: Box::pin(sleep(SERVER_TIMEOUT_INTERVAL)),
                };
            }
            Self::Connected { timeout, .. } => {
                timeout
                    .as_mut()
                    .reset(Instant::now() + SERVER_TIMEOUT_INTERVAL);
            }
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4};

    use dungeon_vr_cryptography::PrivateKey;
    use dungeon_vr_shared::protocol::challenge_token::ChallengeToken;
    use dungeon_vr_shared::protocol::connect_challenge_packet::ConnectChallengePacket;
    use dungeon_vr_shared::protocol::packet::Packet;
    use dungeon_vr_shared::protocol::sealed::Sealed;
    use dungeon_vr_shared::protocol::{GAME_ID, SAFE_RECV_BUFFER_SIZE};
    use dungeon_vr_stream_codec::StreamCodec;
    use tokio::net::UdpSocket;

    use super::Client;

    #[tokio::test(start_paused = true)]
    async fn success() {
        let server_socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let server_addr = server_socket.local_addr().unwrap();
        let client = Client::spawn(server_addr).await.unwrap();
        let mut buf = [0; SAFE_RECV_BUFFER_SIZE];

        // Receive a ConnectInit packet.
        let (size, peer) = server_socket.recv_from(&mut buf[..]).await.unwrap();
        let packet = Packet::read_from(&mut &buf[..size]).unwrap();
        let packet = match packet {
            Packet::ConnectInit(packet) => packet,
            _ => panic!(),
        };
        println!("Received ConnectInit packet");
        assert_eq!(GAME_ID, packet.game_id);
        let client_public_key = packet.client_public_key;

        // Send a ConnectChallenge packet.
        let server_private_key = PrivateKey::gen();
        let server_public_key = server_private_key.to_public();
        let shared_secret = server_private_key.exchange(&client_public_key).unwrap();
        let token = ChallengeToken::gen();
        let mut w = Vec::new();
        Packet::ConnectChallenge(ConnectChallengePacket {
            server_public_key,
            sealed_payload: Sealed::seal(token, &shared_secret),
        })
        .write_to(&mut w)
        .unwrap();
        server_socket.send_to(&w, peer).await.unwrap();
        println!("Sent ConnectChallenge packet");

        // Receive a ConnectResponse packet.
        let (size, _) = server_socket.recv_from(&mut buf[..]).await.unwrap();
        let packet = Packet::read_from(&mut &buf[..size]).unwrap();
        let response_token = match packet {
            Packet::ConnectResponse(packet) => packet.open(&shared_secret).unwrap(),
            _ => panic!(),
        };
        println!("Received ConnectResponse packet");
        assert_eq!(token, response_token);

        client.shutdown().await.unwrap();
    }
}
