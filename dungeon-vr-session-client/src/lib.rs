use std::collections::{BTreeMap, HashMap};
use std::future::pending;

use dungeon_vr_connection_client::{
    ConnectionState, Event as ConnectionEvent, Request as ConnectionRequest,
};
use dungeon_vr_session_shared::action::Action;
use dungeon_vr_session_shared::core::NetId;
use dungeon_vr_session_shared::packet::commit_actions_packet::CommitActionsPacket;
use dungeon_vr_session_shared::packet::game_state_packet::GameStatePacket;
use dungeon_vr_session_shared::packet::ping_packet::PingPacket;
use dungeon_vr_session_shared::packet::player_assignment_packet::PlayerAssignmentPacket;
use dungeon_vr_session_shared::packet::pong_packet::PongPacket;
use dungeon_vr_session_shared::packet::update_owned_transforms_packet::UpdateOwnedTransformsPacket;
use dungeon_vr_session_shared::packet::voice_packet::VoicePacket;
use dungeon_vr_session_shared::packet::Packet;
use dungeon_vr_session_shared::time::{ClientTime, ClientTokioEpoch, NanoDuration, TokioEpoch};
use dungeon_vr_session_shared::{PlayerId, TickId};
use dungeon_vr_stream_codec::StreamCodec;
use rapier3d::prelude::*;
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::sleep_until;

const EVENT_BUFFER_SIZE: usize = 256;
const REQUEST_BUFFER_SIZE: usize = 256;
const PING_INTERVAL: NanoDuration = NanoDuration::from_nanos(100_000_000);
const PING_SAMPLES: usize = 10;
const INITIAL_SLACK: NanoDuration = NanoDuration::from_nanos(100_000_000);

pub struct SessionClient {
    _cancel_guard: cancel::Guard,
    events: mpsc::Receiver<Event>,
    requests: mpsc::Sender<Request>,
}

enum InternalEvent {
    Connection(Option<ConnectionEvent>),
    State(StateEvent),
    Request(Option<Request>),
}

impl SessionClient {
    pub fn new(
        connection_requests: mpsc::Sender<ConnectionRequest>,
        connection_events: mpsc::Receiver<ConnectionEvent>,
    ) -> Self {
        let cancel_token = cancel::Token::new();
        let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let (request_tx, request_rx) = mpsc::channel(REQUEST_BUFFER_SIZE);
        tokio::spawn(
            InnerClient::new(
                cancel_token.clone(),
                connection_requests,
                connection_events,
                event_tx,
                request_rx,
            )
            .run(),
        );
        Self {
            _cancel_guard: cancel_token.guard(),
            events: event_rx,
            requests: request_tx,
        }
    }

    pub fn try_recv_event(&mut self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub async fn recv_event(&mut self) -> Event {
        self.events.recv().await.unwrap()
    }

    pub fn try_send_request(&self, request: Request) -> Result<(), ()> {
        self.requests.try_send(request).map_err(|_| ())
    }
}

struct InnerClient {
    cancel_token: cancel::Token,
    connection_requests: mpsc::Sender<ConnectionRequest>,
    connection_events: mpsc::Receiver<ConnectionEvent>,
    events: mpsc::Sender<Event>,
    requests: mpsc::Receiver<Request>,
    epoch: ClientTokioEpoch,
    state: State,
}

#[derive(Debug)]
enum State {
    AwaitingConnection,
    MeasuringPing {
        local_player_id: Option<PlayerId>,
        next_ping_time: ClientTime,
        rtt_samples: Vec<NanoDuration>,
        server_last_completed_tick: Option<TickId>,
        server_tick_interval: Option<NanoDuration>,
    },
    Running,
}

enum StateEvent {
    PingElapsed,
}

impl State {
    async fn event(&mut self, epoch: &ClientTokioEpoch) -> StateEvent {
        match self {
            Self::AwaitingConnection => pending().await,
            Self::MeasuringPing {
                local_player_id,
                next_ping_time,
                rtt_samples,
                server_last_completed_tick,
                server_tick_interval,
            } => {
                // TODO: Use select!{} and enforce a timeout.
                sleep_until(epoch.instant_at(*next_ping_time)).await;
                StateEvent::PingElapsed
            }
            Self::Running => pending().await,
        }
    }
}

pub enum Event {
    Start {
        local_player_id: PlayerId,
        tick_id: TickId,
    },
    Snapshot {
        tick_id: TickId,
        tick_interval: NanoDuration,
        data: Vec<u8>,
    },
    Voice(Vec<u8>),
}

pub enum Request {
    SendVoice(Vec<u8>),
    CommitActions(BTreeMap<TickId, Vec<Action>>),
    UpdateOwnedTransforms(HashMap<NetId, Isometry<f32>>),
}

impl InnerClient {
    fn new(
        cancel_token: cancel::Token,
        connection_requests: mpsc::Sender<ConnectionRequest>,
        connection_events: mpsc::Receiver<ConnectionEvent>,
        events: mpsc::Sender<Event>,
        requests: mpsc::Receiver<Request>,
    ) -> Self {
        log::info!("Session state: awaiting connection");
        Self {
            cancel_token: cancel_token.clone(),
            connection_requests,
            connection_events,
            events,
            requests,
            epoch: TokioEpoch::new(),
            state: State::AwaitingConnection,
        }
    }

    async fn run(mut self) {
        while !self.cancel_token.is_cancelled() {
            let state_event = self.state.event(&self.epoch);

            let event = select! {
                biased;

                _ = self.cancel_token.cancelled() => break,

                event = self.connection_events.recv() => InternalEvent::Connection(event),

                event = state_event => InternalEvent::State(event),

                request = self.requests.recv() => InternalEvent::Request(request),
            };

            match event {
                InternalEvent::Connection(event) => self.handle_connection_event(event).await,
                InternalEvent::State(event) => self.handle_state_event(event).await,
                InternalEvent::Request(request) => self.handle_request(request).await,
            }
        }

        // Wait for the connection to drop.
        loop {
            match self.connection_events.recv().await {
                Some(ConnectionEvent::Dropped) => break,
                Some(_) => (),
                None => {
                    log::error!("Connection closed its event channel before signaling Dropped");
                }
            }
        }
    }

    async fn handle_connection_event(&mut self, event: Option<ConnectionEvent>) {
        match event.unwrap() {
            ConnectionEvent::State(state) => self.handle_connection_state(state),
            ConnectionEvent::GameData(data) => self.handle_connection_game_data(data).await,
            ConnectionEvent::Dropped => self.handle_connection_dropped(),
        }
    }

    fn handle_connection_state(&mut self, state: ConnectionState) {
        match state {
            ConnectionState::Disconnected => {
                self.cancel_token.cancel();
            }
            ConnectionState::Connecting => (),
            ConnectionState::Responding => (),
            ConnectionState::Connected => {
                assert!(matches!(self.state, State::AwaitingConnection));
                log::info!("Session state: measuring ping");
                self.state = State::MeasuringPing {
                    local_player_id: None,
                    next_ping_time: self.epoch.now() + PING_INTERVAL,
                    rtt_samples: Vec::new(),
                    server_last_completed_tick: None,
                    server_tick_interval: None,
                };
            }
        }
    }

    async fn handle_connection_game_data(&mut self, data: Vec<u8>) {
        let mut r = data.as_slice();
        let packet = match Packet::read_from(&mut r) {
            Ok(packet) => packet,
            Err(e) => {
                log::error!("Error decoding game data packet: {e}");
                return;
            }
        };
        if !r.is_empty() {
            log::error!(
                "Dropping {:?} game data packet: {} unexpected trailing byte(s)",
                packet.kind(),
                r.len(),
            );
            return;
        }
        match packet {
            Packet::Pong(packet) => self.handle_pong_packet(packet).await,
            Packet::GameState(packet) => self.handle_game_state_packet(packet).await,
            Packet::Voice(packet) => self.handle_voice_packet(packet).await,
            Packet::PlayerAssignment(packet) => self.handle_player_assignment_packet(packet).await,
            _ => {
                log::error!("Unexpected game data packet: {:?}", packet.kind());
            }
        }
    }

    async fn handle_pong_packet(&mut self, packet: PongPacket) {
        if let State::MeasuringPing {
            rtt_samples,
            server_last_completed_tick,
            server_tick_interval,
            ..
        } = &mut self.state
        {
            let now = self.epoch.now();
            let round_trip_time = now - packet.client_time;

            rtt_samples.push(round_trip_time);
            *server_last_completed_tick = Some(packet.server_last_completed_tick);
            *server_tick_interval = Some(packet.server_tick_interval);
        } else {
            log::warn!("Dropping unexpected pong packet")
        }
    }

    async fn handle_game_state_packet(&mut self, packet: GameStatePacket) {
        if matches!(self.state, State::Running) {
            send_event(
                &self.events,
                Event::Snapshot {
                    tick_id: packet.tick_id,
                    tick_interval: packet.tick_interval,
                    data: packet.serialized_game_state,
                },
            )
            .await;
        } else {
            log::warn!("Dropping unexpected game state packet");
        }
    }

    async fn handle_voice_packet(&mut self, packet: VoicePacket) {
        send_event(&self.events, Event::Voice(packet.data)).await;
    }

    async fn handle_player_assignment_packet(&mut self, packet: PlayerAssignmentPacket) {
        if let State::MeasuringPing {
            local_player_id: local_player_id @ None,
            ..
        } = &mut self.state
        {
            log::info!("Accepted player assignment: {}", packet.player_id);
            *local_player_id = Some(packet.player_id);
        }
    }

    fn handle_connection_dropped(&mut self) {
        todo!()
    }

    async fn handle_state_event(&mut self, event: StateEvent) {
        match &mut self.state {
            State::AwaitingConnection => unreachable!(),
            State::MeasuringPing {
                local_player_id,
                next_ping_time,
                rtt_samples,
                server_last_completed_tick,
                server_tick_interval,
            } => match event {
                StateEvent::PingElapsed => {
                    if rtt_samples.len() >= PING_SAMPLES {
                        let server_tick_interval = server_tick_interval.unwrap();

                        // Compute an RTT estimate, add a fixed amount of slack, and convert that to
                        // a tick ID. This should be close to where the server will be trying to
                        // maintain our sync point.
                        let rtt = rtt_samples.iter().copied().sum::<NanoDuration>()
                            / rtt_samples.len() as i64;
                        let ticks_ahead =
                            (rtt + INITIAL_SLACK + server_tick_interval / 2) / server_tick_interval;
                        let tick_id = TickId(
                            server_last_completed_tick
                                .unwrap()
                                .0
                                .wrapping_add(ticks_ahead as u32),
                        );

                        log::info!(
                            "Initial RTT estimate {} ns; starting at tick {}",
                            rtt.as_nanos(),
                            tick_id.0,
                        );
                        send_event(
                            &self.events,
                            Event::Start {
                                local_player_id: local_player_id.unwrap(),
                                tick_id,
                            },
                        )
                        .await;

                        log::info!("Session state: running");
                        self.state = State::Running;
                    } else {
                        // Need more samples. There may already be enough packets in flight, but
                        // send another ping.
                        *next_ping_time += PING_INTERVAL;
                        send_packet(
                            &self.connection_requests,
                            Packet::Ping(PingPacket {
                                client_time: self.epoch.now(),
                            }),
                        )
                        .await;
                    }
                }
            },
            State::Running => unreachable!(),
        }
    }

    async fn handle_request(&mut self, request: Option<Request>) {
        match request.unwrap() {
            Request::SendVoice(data) => {
                send_packet(
                    &self.connection_requests,
                    Packet::Voice(VoicePacket { data }),
                )
                .await;
            }
            Request::CommitActions(actions_by_tick_id) => {
                // TODO: Record committed actions and send them redundantly until acknowledged by
                // the server. This is the simplest thing that can work, but it drops inputs on a
                // single lost packet.
                send_packet(
                    &self.connection_requests,
                    Packet::CommitActions(CommitActionsPacket { actions_by_tick_id }),
                )
                .await;
            }
            Request::UpdateOwnedTransforms(transforms_by_net_id) => {
                send_packet(
                    &self.connection_requests,
                    Packet::UpdateOwnedTransforms(UpdateOwnedTransformsPacket {
                        after_tick_id: TickId(0), // TODO
                        transforms_by_net_id,
                    }),
                )
                .await;
            }
        }
    }
}

async fn send_event(events: &mpsc::Sender<Event>, event: Event) {
    let _ = events.send(event).await;
}

async fn send_packet(requests: &mpsc::Sender<ConnectionRequest>, packet: Packet) {
    let mut data = Vec::new();
    packet.write_to(&mut data).unwrap();
    let _ = requests.send(ConnectionRequest::SendGameData(data)).await;
}
