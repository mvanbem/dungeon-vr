use std::collections::{hash_map, HashMap};
use std::iter::repeat_with;

use dungeon_vr_connection_server::{
    ConnectionState, Event as ConnectionEvent, Request as ConnectionRequest,
};
use dungeon_vr_session_shared::net_game::{NetGame, NetGameFullCodec, PlayerId};
use dungeon_vr_session_shared::packet::game_state_packet::GameStatePacket;
use dungeon_vr_session_shared::packet::ping_packet::PingPacket;
use dungeon_vr_session_shared::packet::pong_packet::PongPacket;
use dungeon_vr_session_shared::packet::voice_packet::VoicePacket;
use dungeon_vr_session_shared::packet::{Packet, TickId};
use dungeon_vr_session_shared::time::ServerEpoch;
use dungeon_vr_socket::AddrBound;
use dungeon_vr_stream_codec::{ExternalStreamCodec, StreamCodec};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, Interval};

const TICK_INTERVAL: Duration = Duration::from_millis(50);

trait PlayerIdExt {
    fn index(self) -> usize;
    fn from_index(index: usize) -> Self;
}

impl PlayerIdExt for PlayerId {
    fn index(self) -> usize {
        (self.0.get() - 1) as usize
    }

    fn from_index(index: usize) -> Self {
        Self(u8::try_from(index + 1).unwrap().try_into().unwrap())
    }
}

pub struct SessionServer {
    _cancel_guard: cancel::Guard,
}

enum Event<Addr> {
    Connection(Option<ConnectionEvent<Addr>>),
    Tick,
}

impl SessionServer {
    pub fn new<Addr: AddrBound>(
        connection_requests: mpsc::Sender<ConnectionRequest<Addr>>,
        connection_events: mpsc::Receiver<ConnectionEvent<Addr>>,
        max_players: usize,
    ) -> Self {
        let cancel_token = cancel::Token::new();
        tokio::spawn(
            InnerServer::new(
                cancel_token.clone(),
                connection_requests,
                connection_events,
                max_players,
            )
            .run(),
        );
        Self {
            _cancel_guard: cancel_token.guard(),
        }
    }
}

struct InnerServer<Addr> {
    cancel_token: cancel::Token,
    connection_requests: mpsc::Sender<ConnectionRequest<Addr>>,
    connection_events: mpsc::Receiver<ConnectionEvent<Addr>>,
    clients: HashMap<Addr, ClientState>,
    players: Vec<Option<PlayerState<Addr>>>,
    epoch: ServerEpoch,
    game: NetGame,
    tick_interval: Interval,
    next_tick_id: TickId,
}

struct ClientState {
    player_id: Option<PlayerId>,
}

struct PlayerState<Addr> {
    addr: Addr,
}

impl<Addr: AddrBound> InnerServer<Addr> {
    fn new(
        cancel_token: cancel::Token,
        connection_requests: mpsc::Sender<ConnectionRequest<Addr>>,
        connection_events: mpsc::Receiver<ConnectionEvent<Addr>>,
        max_players: usize,
    ) -> Self {
        Self {
            cancel_token,
            connection_requests,
            connection_events,
            clients: HashMap::new(),
            players: repeat_with(|| None).take(max_players).collect(),
            epoch: ServerEpoch::new(),
            game: NetGame::new(),
            tick_interval: interval(TICK_INTERVAL),
            next_tick_id: TickId(0),
        }
    }

    async fn run(mut self) {
        while !self.cancel_token.is_cancelled() {
            let event = select! {
                biased;

                event = self.connection_events.recv() => Event::Connection(event),

                _ = self.tick_interval.tick() => Event::Tick,
            };

            match event {
                Event::Connection(event) => self.handle_connection_event(event.unwrap()).await,
                Event::Tick => self.handle_tick().await,
            }
        }
    }

    async fn handle_connection_event(&mut self, event: ConnectionEvent<Addr>) {
        match event {
            ConnectionEvent::State { addr, state } => self.handle_connection_state(addr, state),
            ConnectionEvent::GameData { addr, data } => {
                self.handle_connection_game_data(addr, data).await
            }
            ConnectionEvent::Dropped => self.handle_connection_dropped(),
        }
    }

    fn handle_connection_state(&mut self, addr: Addr, state: ConnectionState) {
        match state {
            ConnectionState::Disconnected => {
                let client_entry = match self.clients.entry(addr) {
                    hash_map::Entry::Occupied(entry) => entry,
                    _ => unreachable!(),
                };
                // Connections may or may not pass through the Disconnecting state on their way to
                // Disconnected, so there might still be a player mapping.
                if let Some(player_id) = client_entry.get().player_id {
                    log::info!("{player_id} disconnected");
                    self.players[player_id.index()] = None;
                }
                client_entry.remove();
            }
            ConnectionState::Pending => {
                let prev = self.clients.insert(addr, ClientState { player_id: None });
                assert!(prev.is_none());
            }
            ConnectionState::Connected => {
                let client = self.clients.get_mut(&addr).unwrap();
                match self.players.iter().position(Option::is_none) {
                    Some(index) => {
                        let player_id = PlayerId::from_index(index);
                        log::info!("Peer {addr} connected as {player_id}");
                        self.players[index] = Some(PlayerState { addr });
                        client.player_id = Some(player_id);
                    }
                    None => {
                        log::info!("Peer {addr} connected, but the server is full");
                        // TODO: Kick the player? Approve connections before they are made? Just let
                        // them spectate?
                    }
                }
            }
            ConnectionState::Disconnecting => {
                if let Some(player_id) = self.clients[&addr].player_id {
                    log::info!("{player_id} disconnected");
                    self.players[player_id.index()] = None;
                }
            }
        }
    }

    async fn handle_connection_game_data(&mut self, addr: Addr, data: Vec<u8>) {
        let mut r = data.as_slice();
        let packet = match Packet::read_from(&mut r) {
            Ok(packet) => packet,
            Err(e) => {
                log::error!("Error decoding game data packet from client {addr}: {e}");
                return;
            }
        };
        if !r.is_empty() {
            log::error!(
                "Client {addr}: Dropping {:?} game data packet: {} unexpected trailing byte(s)",
                packet.kind(),
                r.len(),
            );
            return;
        }
        match packet {
            Packet::Ping(packet) => self.handle_ping_packet(addr, packet).await,
            Packet::Voice(packet) => self.handle_voice_packet(addr, packet).await,
            _ => {
                log::error!("Unexpected game data packet: {:?}", packet.kind());
            }
        }
    }

    async fn handle_ping_packet(&mut self, addr: Addr, packet: PingPacket) {
        send_game_data(
            &self.connection_requests,
            addr,
            Packet::Pong(PongPacket {
                client_time: packet.client_time,
                server_time: self.epoch.now(),
            }),
        )
        .await;
    }

    async fn handle_voice_packet(&mut self, addr: Addr, packet: VoicePacket) {
        for player in self.players.iter().flatten() {
            // Forward voice packets to all other players.
            // TODO: Tag voice packets with player numbers so they can be reconstructed into
            // separate streams.
            if player.addr != addr {
                send_game_data(
                    &self.connection_requests,
                    player.addr,
                    Packet::Voice(VoicePacket {
                        data: packet.data.clone(),
                    }),
                )
                .await;
            }
        }
    }

    fn handle_connection_dropped(&mut self) {
        todo!()
    }

    async fn handle_tick(&mut self) {
        let tick_id = self.take_next_tick_id();

        // Gather the current buffered inputs for this tick from each connection.
        // let player_inputs = self.players.iter().map(|x| x.buffered_input).collect();
        self.game.update(std::collections::BTreeMap::default());

        // Send updates to all players.
        for player in self.players.iter().flatten() {
            let mut serialized_game_state = Vec::new();
            NetGameFullCodec::write_to_ext(&mut serialized_game_state, &self.game).unwrap();
            send_game_data(
                &self.connection_requests,
                player.addr,
                Packet::GameState(GameStatePacket {
                    tick_id,
                    serialized_game_state,
                }),
            )
            .await;
        }
    }

    fn take_next_tick_id(&mut self) -> TickId {
        let tick_id = self.next_tick_id;
        self.next_tick_id = TickId(self.next_tick_id.0 + 1);
        tick_id
    }
}

async fn send_game_data<Addr: AddrBound>(
    connection_requests: &mpsc::Sender<ConnectionRequest<Addr>>,
    addr: Addr,
    packet: Packet,
) {
    let mut data = Vec::new();
    packet.write_to(&mut data).unwrap();
    let _ = connection_requests
        .send(ConnectionRequest::SendGameData { addr, data })
        .await;
}
