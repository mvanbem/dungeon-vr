use std::collections::{btree_map, hash_map, BTreeMap, HashMap};
use std::f32::consts::FRAC_PI_2;
use std::future::pending;
use std::iter::repeat_with;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::time::Duration;

use bevy_ecs::prelude::*;
use dungeon_vr_connection_server::{
    ConnectionState, Event as ConnectionEvent, Request as ConnectionRequest,
};
use dungeon_vr_session_shared::action::{apply_actions, Action};
use dungeon_vr_session_shared::collider_cache::ColliderCache;
use dungeon_vr_session_shared::core::{
    Authority, LocalAuthorityResource, NetId, SynchronizedComponent, TransformComponent,
};
use dungeon_vr_session_shared::fly_around::fly_around;
use dungeon_vr_session_shared::fly_around::FlyAroundComponent;
use dungeon_vr_session_shared::interaction::{GrabbableComponent, HandComponent, HandGrabState};
use dungeon_vr_session_shared::packet::commit_actions_packet::CommitActionsPacket;
use dungeon_vr_session_shared::packet::game_state_packet::GameStatePacket;
use dungeon_vr_session_shared::packet::ping_packet::PingPacket;
use dungeon_vr_session_shared::packet::player_assignment_packet::PlayerAssignmentPacket;
use dungeon_vr_session_shared::packet::pong_packet::PongPacket;
use dungeon_vr_session_shared::packet::update_owned_transforms_packet::UpdateOwnedTransformsPacket;
use dungeon_vr_session_shared::packet::voice_packet::VoicePacket;
use dungeon_vr_session_shared::packet::Packet;
use dungeon_vr_session_shared::physics::{
    reset_forces, step_physics, sync_physics, update_rigid_body_transforms, PhysicsComponent,
    PhysicsResource,
};
use dungeon_vr_session_shared::render::RenderComponent;
use dungeon_vr_session_shared::resources::{AllActionsResource, EntitiesByNetIdResource};
use dungeon_vr_session_shared::snapshot::write_snapshot;
use dungeon_vr_session_shared::time::{NanoDuration, ServerTime, ServerTokioEpoch, TokioEpoch};
use dungeon_vr_session_shared::{PlayerId, TickId, TICK_INTERVAL};
use dungeon_vr_socket::AddrBound;
use dungeon_vr_stream_codec::StreamCodec;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use rapier3d::prelude::*;
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep_until, Interval};

const SEND_ASSIGNMENT_INTERVAL: Duration = Duration::from_millis(250);

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

#[derive(Clone)]
pub struct NetIdAllocator {
    next: NetId,
}

impl NetIdAllocator {
    pub fn new() -> Self {
        Self {
            next: NetId(NonZeroU32::new(1).unwrap()),
        }
    }

    pub fn next(&mut self) -> NetId {
        let result = self.next;
        self.next = NetId(self.next.0.checked_add(1).unwrap());
        result
    }
}

pub struct SessionServer {
    _cancel_guard: cancel::Guard,
}

enum Event<Addr> {
    Connection(Option<ConnectionEvent<Addr>>),
    PlayerEvent(PlayerEvent),
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
    epoch: ServerTokioEpoch,
    world: World,
    tick_schedule: Schedule,
    net_ids: NetIdAllocator,
    /// The ID of the most recently completed tick.
    last_completed_tick_id: TickId,
    /// When the next tick is scheduled.
    next_tick_time: ServerTime,
}

struct ClientState {
    player_id: Option<PlayerId>,
}

struct PlayerState<Addr> {
    addr: Addr,
    send_assignment: Option<Pin<Box<Interval>>>,
    committed_actions_by_tick_id: BTreeMap<TickId, CommittedActions>,
    slack_estimate_nanoseconds: f64,
}

impl<Addr> PlayerState<Addr> {
    fn record_slack_observation(&mut self, slack: NanoDuration) {
        self.slack_estimate_nanoseconds =
            0.99 * self.slack_estimate_nanoseconds + 0.01 * slack.as_nanos() as f64;
    }
}

struct CommittedActions {
    slack: NanoDuration,
    actions: Vec<Action>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, StageLabel)]
enum StageLabel {
    Singleton,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SystemLabel)]
enum SystemLabel {
    Init,
    CoreTick,
    UpdateBeforePhysics,
    PhysicsStep,
    UpdateAfterPhysics,
}

impl<Addr: AddrBound> InnerServer<Addr> {
    fn new(
        cancel_token: cancel::Token,
        connection_requests: mpsc::Sender<ConnectionRequest<Addr>>,
        connection_events: mpsc::Receiver<ConnectionEvent<Addr>>,
        max_players: usize,
    ) -> Self {
        let mut world = World::new();
        let mut net_ids = NetIdAllocator::new();
        let mut entities_by_net_id = EntitiesByNetIdResource::default();

        // Spawn world geometry.
        let mut spawn_context = SpawnContext {
            world: &mut world,
            net_ids: &mut net_ids,
            entities_by_net_id: &mut entities_by_net_id,
        };
        spawn_context.spawn_static_model(
            "LowPolyDungeon/Dungeon_Custom_Center",
            vector![0.0, 0.0, 0.0].into(),
        );
        for side in 0..4 {
            let rot = Rotation::from_scaled_axis(vector![0.0, FRAC_PI_2, 0.0] * side as f32);
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Custom_Border_Flat",
                Isometry::from_parts((rot * vector![0.0, 0.0, -4.0]).into(), rot),
            );
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Wall_Var1",
                Isometry::from_parts((rot * vector![0.0, 0.0, -4.0]).into(), rot),
            );

            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Custom_Corner_Flat",
                Isometry::from_parts((rot * vector![4.0, 0.0, 4.0]).into(), rot),
            );
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Wall_Var1",
                Isometry::from_parts((rot * vector![-4.0, 0.0, -4.0]).into(), rot),
            );
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Wall_Var1",
                Isometry::from_parts((rot * vector![4.0, 0.0, -4.0]).into(), rot),
            );
        }

        // Spawn a rotating sword as a test object.
        let net_id = net_ids.next();
        entities_by_net_id.0.insert(
            net_id,
            world
                .spawn()
                .insert(SynchronizedComponent {
                    net_id,
                    authority: Authority::Server,
                })
                .insert(TransformComponent::default())
                .insert(RenderComponent::new("LowPolyDungeon/Sword"))
                .insert(FlyAroundComponent)
                .id(),
        );

        // Spawn a few grabbable keys.
        let net_id = net_ids.next();
        entities_by_net_id.0.insert(
            net_id,
            world
                .spawn()
                .insert(SynchronizedComponent {
                    net_id,
                    authority: Authority::Server,
                })
                .insert(TransformComponent(vector![0.0, 1.0, 0.0].into()))
                .insert(RenderComponent::new("LowPolyDungeon/Key_Silver"))
                .insert(GrabbableComponent { grabbed: false })
                .insert(PhysicsComponent::new_dynamic_ccd(
                    "LowPolyDungeon/Key_Silver",
                ))
                .id(),
        );
        let net_id = net_ids.next();
        entities_by_net_id.0.insert(
            net_id,
            world
                .spawn()
                .insert(SynchronizedComponent {
                    net_id,
                    authority: Authority::Server,
                })
                .insert(TransformComponent(vector![0.5, 1.0, 0.0].into()))
                .insert(RenderComponent::new("LowPolyDungeon/Key_Silver"))
                .insert(GrabbableComponent { grabbed: false })
                .insert(PhysicsComponent::new_dynamic_ccd(
                    "LowPolyDungeon/Key_Silver",
                ))
                .id(),
        );
        let net_id = net_ids.next();
        entities_by_net_id.0.insert(
            net_id,
            world
                .spawn()
                .insert(SynchronizedComponent {
                    net_id,
                    authority: Authority::Server,
                })
                .insert(TransformComponent(vector![0.0, 1.0, 0.5].into()))
                .insert(RenderComponent::new("LowPolyDungeon/Key_Silver"))
                .insert(GrabbableComponent { grabbed: false })
                .insert(PhysicsComponent::new_dynamic_ccd(
                    "LowPolyDungeon/Key_Silver",
                ))
                .id(),
        );

        world.insert_resource(PhysicsResource::new(
            RigidBodySet::new(),
            ColliderSet::new(),
            ColliderCache::new(),
            TICK_INTERVAL.as_secs_f32(),
        ));
        world.insert_resource(entities_by_net_id);
        world.insert_resource(LocalAuthorityResource(Some(Authority::Server)));

        let epoch = TokioEpoch::new();
        Self {
            cancel_token,
            connection_requests,
            connection_events,
            clients: HashMap::new(),
            players: repeat_with(|| None).take(max_players).collect(),
            epoch,
            world,
            tick_schedule: Schedule::default().with_stage(
                StageLabel::Singleton,
                SystemStage::parallel()
                    .with_system_set(
                        SystemSet::new()
                            .label(SystemLabel::Init)
                            .with_system(reset_forces)
                            .with_system(sync_physics),
                    )
                    .with_system_set(
                        SystemSet::new()
                            .after(SystemLabel::Init)
                            .label(SystemLabel::CoreTick)
                            .with_system(apply_actions),
                    )
                    .with_system_set(
                        SystemSet::new()
                            .after(SystemLabel::CoreTick)
                            .label(SystemLabel::UpdateBeforePhysics)
                            .with_system(fly_around),
                    )
                    .with_system_set(
                        SystemSet::new()
                            .after(SystemLabel::UpdateBeforePhysics)
                            .label(SystemLabel::PhysicsStep)
                            .with_system(step_physics),
                    )
                    .with_system_set(
                        SystemSet::new()
                            .after(SystemLabel::PhysicsStep)
                            .label(SystemLabel::UpdateAfterPhysics)
                            .with_system(update_rigid_body_transforms),
                    ),
            ),
            net_ids,
            last_completed_tick_id: TickId(0),
            next_tick_time: epoch.now() + TICK_INTERVAL,
        }
    }

    async fn run(mut self) {
        while !self.cancel_token.is_cancelled() {
            let mut dynamic_events = FuturesUnordered::from_iter(
                iter_players_mut(&mut self.players).map(|(player_id, player)| async move {
                    Event::PlayerEvent(player.wait_for_event(player_id).await)
                }),
            );
            let tick = sleep_until(self.epoch.instant_at(self.next_tick_time));

            let event = select! {
                biased;

                event = self.connection_events.recv() => Event::Connection(event),

                Some(event) = dynamic_events.next() => event,

                _ = tick => Event::Tick,
            };
            drop(dynamic_events);

            match event {
                Event::Connection(event) => self.handle_connection_event(event.unwrap()).await,
                Event::PlayerEvent(event) => self.handle_player_event(event).await,
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

    async fn handle_player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::SendAssignment { player_id } => {
                let player = self.players[player_id.index()].as_mut().unwrap();
                send_game_data(
                    &self.connection_requests,
                    player.addr,
                    Packet::PlayerAssignment(PlayerAssignmentPacket { player_id }),
                )
                .await;
            }
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
                let player_id = client_entry.get().player_id;
                if let Some(player_id) = player_id {
                    log::info!("{player_id} disconnected");
                    self.players[player_id.index()] = None;
                }
                client_entry.remove();
                if let Some(player_id) = player_id {
                    self.despawn_player(player_id);
                }
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
                        self.players[index] = Some(PlayerState {
                            addr,
                            // Prepare to tell this player their player ID assignment repeatedly
                            // until we process a session packet indicating they got the message.
                            send_assignment: Some(Box::pin(interval(SEND_ASSIGNMENT_INTERVAL))),
                            committed_actions_by_tick_id: BTreeMap::new(),
                            slack_estimate_nanoseconds: 0.0,
                        });
                        *client = ClientState {
                            player_id: Some(player_id),
                        };

                        self.spawn_player(player_id);
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
                    self.despawn_player(player_id);
                }
            }
        }
    }

    fn spawn_player(&mut self, player_id: PlayerId) {
        log::info!("Spawning hands for {player_id}");
        for index in 0..2 {
            let net_id = self.net_ids.next();
            let entity = self
                .world
                .spawn()
                .insert(SynchronizedComponent {
                    net_id,
                    authority: Authority::Player(player_id),
                })
                .insert(TransformComponent::default())
                .insert(RenderComponent::new(["left_hand", "right_hand"][index]))
                .insert(HandComponent {
                    index,
                    grab_state: HandGrabState::Empty,
                })
                .id();
            self.world
                .resource_mut::<EntitiesByNetIdResource>()
                .0
                .insert(net_id, entity);
        }
    }

    fn despawn_player(&mut self, player_id: PlayerId) {
        // Despawn any hands owned by the player.
        let owned_hands = Vec::from_iter(
            self.world
                .query_filtered::<(Entity, &SynchronizedComponent), With<HandComponent>>()
                .iter(&self.world)
                .filter_map(|(entity, synchronized)| {
                    if synchronized.authority == Authority::Player(player_id) {
                        Some((entity, synchronized.net_id))
                    } else {
                        None
                    }
                }),
        );
        log::info!("Despawning {} hands for {player_id}", owned_hands.len());
        for (entity, net_id) in owned_hands {
            self.world.despawn(entity);
            self.world
                .resource_mut::<EntitiesByNetIdResource>()
                .0
                .remove(&net_id);
        }

        // Transfer any other entities owned by the player back to server authority.
        let mut count = 0usize;
        for mut synchronized in self
            .world
            .query::<&mut SynchronizedComponent>()
            .iter_mut(&mut self.world)
        {
            if synchronized.authority == Authority::Player(player_id) {
                synchronized.authority = Authority::Server;
                count += 1;
            }
        }
        log::info!("Reclaimed {count} entities from {player_id}");
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
            Packet::CommitActions(packet) => self.handle_commit_actions_packet(addr, packet).await,
            Packet::UpdateOwnedTransforms(packet) => {
                self.handle_update_owned_transforms_packet(addr, packet)
                    .await
            }
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
                server_last_completed_tick: self.last_completed_tick_id,
                server_tick_interval: TICK_INTERVAL,
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

    async fn handle_commit_actions_packet(&mut self, addr: Addr, packet: CommitActionsPacket) {
        let player_id = match self.clients[&addr].player_id {
            Some(player_id) => player_id,
            None => {
                log::warn!("Client {addr}: Dropping commit actions packet: player ID not assigned");
                return;
            }
        };
        let player = self.players[player_id.index()].as_mut().unwrap();
        for (tick_id, actions) in packet.actions_by_tick_id {
            match player.committed_actions_by_tick_id.entry(tick_id) {
                btree_map::Entry::Occupied(_) => {
                    // Actions have already been committed for this tick. This is expected to happen
                    // frequently due to eager over-sending.
                }
                btree_map::Entry::Vacant(entry) => {
                    // Compute when this tick would have happened or will happen.
                    let tick_delta_from_next =
                        tick_id.0 as i64 - (self.last_completed_tick_id.0 as i64 + 1);
                    let time_delta_from_next = TICK_INTERVAL * tick_delta_from_next;
                    let tick_time = self.next_tick_time + time_delta_from_next;
                    // Slack is positive when actions are committed early and negative when they are
                    // committed late.
                    let slack = tick_time - self.epoch.now();

                    if tick_id > self.last_completed_tick_id {
                        // Commits that arrive ahead of time are stored for the upcoming tick.
                        entry.insert(CommittedActions { slack, actions });
                    } else {
                        // Commits that arrive late only affect the slack estimate.
                        player.record_slack_observation(slack);
                    }
                }
            }
        }
    }

    async fn handle_update_owned_transforms_packet(
        &mut self,
        addr: Addr,
        packet: UpdateOwnedTransformsPacket,
    ) {
        let player_id = match self.clients[&addr].player_id {
            Some(player_id) => player_id,
            None => {
                log::warn!("Client {addr}: Dropping commit actions packet: player ID not assigned");
                return;
            }
        };
        // TODO: Use the tick ID in the packet to prevent late packets from introducing excessive
        // jitter. Also everyone should be interpolating.
        for (net_id, transform) in packet.transforms_by_net_id {
            if let Some(&entity) = self
                .world
                .resource::<EntitiesByNetIdResource>()
                .0
                .get(&net_id)
            {
                if let Some(synchronized) = self.world.get::<SynchronizedComponent>(entity) {
                    if synchronized.authority == Authority::Player(player_id) {
                        self.world.get_mut::<TransformComponent>(entity).unwrap().0 = transform;
                    }
                }
            }
        }
    }

    fn handle_connection_dropped(&mut self) {
        todo!()
    }

    async fn handle_tick(&mut self) {
        let tick_id = self.last_completed_tick_id.next();
        let tick_time = self.next_tick_time;

        // Gather the current committed actions for this tick from each player.
        self.world.insert_resource(AllActionsResource({
            let mut all_actions = HashMap::new();
            for (player_id, player) in iter_players_mut(&mut self.players) {
                if let Some(committed_actions) = player.committed_actions_by_tick_id.get(&tick_id) {
                    all_actions.insert(player_id, committed_actions.actions.clone());
                    player.record_slack_observation(committed_actions.slack);
                } else {
                    log::warn!("No actions from {player_id} by {tick_id:?} deadline");
                    // TODO: Use this as the timeout criterion.
                }
            }
            all_actions
        }));
        self.tick_schedule.run(&mut self.world);

        self.last_completed_tick_id = tick_id;
        self.next_tick_time += TICK_INTERVAL;

        // Discard obsolete committed actions.
        // TODO: Keep some window of history to use for tuning client send rates.
        // TODO: Record a -WINDOW_SIZE slack observation when a vacant slot goes out of the window.
        for player in self.players.iter_mut().flatten() {
            player
                .committed_actions_by_tick_id
                .retain(|&action_tick_id, _| action_tick_id > tick_id);
        }

        // Send updates to all players.
        let snapshot = {
            let mut w = Vec::new();
            write_snapshot(&mut w, &mut self.world).unwrap();
            w
        };
        for player in self.players.iter().flatten() {
            const GOAL_SLACK_NS: f64 = 100_000_000.0;

            let error_seconds = (player.slack_estimate_nanoseconds - GOAL_SLACK_NS) * 1e-9;
            let error_ticks = error_seconds / TICK_INTERVAL.as_secs_f64();

            // Compute the tick rate that would resolve the error over the next 10 seconds.
            let tick_rate = 1.0 / TICK_INTERVAL.as_secs_f64() - error_ticks as f64 / 10.0;
            let tick_interval = NanoDuration::from_secs_f64(1.0 / tick_rate)
                .clamp(TICK_INTERVAL * 9 / 10, TICK_INTERVAL * 11 / 10);
            log::debug!(
                "Slack estimate {:.3} ms; assigning client tick interval {:.3} ns",
                player.slack_estimate_nanoseconds * 1e-6,
                tick_interval.as_nanos(),
            );

            send_game_data(
                &self.connection_requests,
                player.addr,
                Packet::GameState(GameStatePacket {
                    tick_id,
                    tick_interval,
                    serialized_game_state: snapshot.clone(),
                }),
            )
            .await;
        }
    }
}

fn iter_players<Addr>(
    players: &Vec<Option<PlayerState<Addr>>>,
) -> impl Iterator<Item = (PlayerId, &PlayerState<Addr>)> {
    players.iter().enumerate().filter_map(|(index, player)| {
        player
            .as_ref()
            .map(|player| (PlayerId::from_index(index), player))
    })
}

fn iter_players_mut<Addr>(
    players: &mut Vec<Option<PlayerState<Addr>>>,
) -> impl Iterator<Item = (PlayerId, &mut PlayerState<Addr>)> {
    players
        .iter_mut()
        .enumerate()
        .filter_map(|(index, player)| {
            player
                .as_mut()
                .map(|player| (PlayerId::from_index(index), player))
        })
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

struct SpawnContext<'a> {
    world: &'a mut World,
    net_ids: &'a mut NetIdAllocator,
    entities_by_net_id: &'a mut EntitiesByNetIdResource,
}

impl<'a> SpawnContext<'a> {
    fn spawn_static_model(&mut self, name: &str, transform: Isometry<f32>) {
        let net_id = self.net_ids.next();
        self.entities_by_net_id.0.insert(
            net_id,
            self.world
                .spawn()
                .insert(SynchronizedComponent {
                    net_id,
                    authority: Authority::Server,
                })
                .insert(TransformComponent(transform))
                .insert(RenderComponent::new(name))
                .insert(PhysicsComponent::new_static(format!("{name}_col")))
                .id(),
        );
    }
}

enum PlayerEvent {
    SendAssignment { player_id: PlayerId },
}

impl<Addr> PlayerState<Addr> {
    async fn wait_for_event(&mut self, player_id: PlayerId) -> PlayerEvent {
        let send_assignment = match &mut self.send_assignment {
            Some(send_assignment) => send_assignment.tick().left_future(),
            None => pending().right_future(),
        };

        select! {
            biased;

            _ = send_assignment => PlayerEvent::SendAssignment { player_id },
        }
    }
}
