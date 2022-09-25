use std::collections::{hash_map, HashMap};
use std::f32::consts::FRAC_PI_2;
use std::iter::repeat_with;
use std::num::NonZeroU32;
use std::pin::Pin;

use bevy_ecs::prelude::*;
use dungeon_vr_connection_server::{
    ConnectionState, Event as ConnectionEvent, Request as ConnectionRequest,
};
use dungeon_vr_session_shared::action::apply_actions;
use dungeon_vr_session_shared::collider_cache::{BorrowedColliderCacheKey, ColliderCache};
use dungeon_vr_session_shared::components::interaction::Grabbable;
use dungeon_vr_session_shared::components::net::{Authority, NetId, Replicated};
use dungeon_vr_session_shared::components::physics::Physics;
use dungeon_vr_session_shared::components::render::ModelName;
use dungeon_vr_session_shared::components::spatial::{FliesAround, Transform};
use dungeon_vr_session_shared::packet::game_state_packet::GameStatePacket;
use dungeon_vr_session_shared::packet::ping_packet::PingPacket;
use dungeon_vr_session_shared::packet::pong_packet::PongPacket;
use dungeon_vr_session_shared::packet::voice_packet::VoicePacket;
use dungeon_vr_session_shared::packet::Packet;
use dungeon_vr_session_shared::resources::{AllActions, EntitiesByNetId};
use dungeon_vr_session_shared::snapshot::write_snapshot;
use dungeon_vr_session_shared::systems::fly_around;
use dungeon_vr_session_shared::time::{LocalEpoch, ServerEpoch};
use dungeon_vr_session_shared::{PlayerId, TickId};
use dungeon_vr_socket::AddrBound;
use dungeon_vr_stream_codec::StreamCodec;
use rapier3d::na::{self as nalgebra, vector, Isometry3, UnitQuaternion};
use rapier3d::prelude::{ColliderSet, RigidBodyBuilder, RigidBodySet};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Sleep};

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
    world: World,
    tick_schedule: Schedule,
    net_ids: NetIdAllocator,
    tick_sleep: Pin<Box<Sleep>>,
    current_tick: TickId,
}

struct ClientState {
    player_id: Option<PlayerId>,
}

struct PlayerState<Addr> {
    addr: Addr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, StageLabel)]
enum StageLabel {
    Singleton,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SystemLabel)]
enum SystemLabel {
    CoreTick,
    UpdateBeforePhysics,
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
        let mut entities_by_net_id = EntitiesByNetId::default();
        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();
        let mut collider_cache = ColliderCache::new();

        // Spawn world geometry.
        let mut spawn_context = SpawnContext {
            world: &mut world,
            colliders: &mut colliders,
            collider_cache: &mut collider_cache,
        };
        spawn_context.spawn_static_model(
            "LowPolyDungeon/Dungeon_Custom_Center",
            vector![0.0, 0.0, 0.0].into(),
        );
        for side in 0..4 {
            let rot = UnitQuaternion::from_scaled_axis(vector![0.0, FRAC_PI_2, 0.0] * side as f32);
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Custom_Border_Flat",
                Isometry3::from_parts((rot * vector![0.0, 0.0, -4.0]).into(), rot),
            );
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Wall_Var1",
                Isometry3::from_parts((rot * vector![0.0, 0.0, -4.0]).into(), rot),
            );

            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Custom_Corner_Flat",
                Isometry3::from_parts((rot * vector![4.0, 0.0, 4.0]).into(), rot),
            );
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Wall_Var1",
                Isometry3::from_parts((rot * vector![-4.0, 0.0, -4.0]).into(), rot),
            );
            spawn_context.spawn_static_model(
                "LowPolyDungeon/Dungeon_Wall_Var1",
                Isometry3::from_parts((rot * vector![4.0, 0.0, -4.0]).into(), rot),
            );
        }

        // Spawn a rotating sword as a test object.
        let net_id = net_ids.next();
        entities_by_net_id.0.insert(
            net_id,
            world
                .spawn()
                .insert_bundle(Replicated {
                    net_id,
                    authority: Authority::Server,
                })
                .insert(Transform::default())
                .insert(ModelName("LowPolyDungeon/Sword".to_string()))
                .insert(FliesAround)
                .id(),
        );

        // Spawn a grabbable key.
        let key_body = bodies.insert(
            RigidBodyBuilder::dynamic()
                .translation(vector![0.0, 1.0, 0.0])
                .ccd_enabled(true)
                .build(),
        );
        let key_collider = colliders.insert_with_parent(
            collider_cache.get(BorrowedColliderCacheKey::ConvexHull(
                "LowPolyDungeon/Key_Silver",
            )),
            key_body,
            &mut bodies,
        );
        world
            .spawn()
            .insert(Transform(vector![0.0, 1.0, 0.0].into()))
            .insert(ModelName("LowPolyDungeon/Key_Silver".to_string()))
            .insert(Grabbable { grabbed: false })
            .insert(Physics {
                collider: key_collider,
                rigid_body: Some(key_body),
            });

        world.insert_resource(entities_by_net_id);

        let epoch = LocalEpoch::new();
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
                            .label(SystemLabel::CoreTick)
                            .with_system(apply_actions),
                    )
                    .with_system_set(
                        SystemSet::new()
                            .after(SystemLabel::CoreTick)
                            .label(SystemLabel::UpdateBeforePhysics)
                            .with_system(fly_around),
                    ),
            ),
            net_ids,
            // NOTE: This is early, but it only affects the very first tick.
            tick_sleep: Box::pin(sleep_until(epoch.instant())),
            current_tick: TickId(0),
        }
    }

    async fn run(mut self) {
        while !self.cancel_token.is_cancelled() {
            let event = select! {
                biased;

                event = self.connection_events.recv() => Event::Connection(event),

                _ = self.tick_sleep.as_mut() => Event::Tick,
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
        self.current_tick = self.current_tick.next();

        // Gather the current buffered inputs for this tick from each connection.
        // let player_inputs = self.players.iter().map(|x| x.buffered_input).collect();
        self.world.insert_resource(self.current_tick);
        self.world.insert_resource(AllActions::default());
        self.tick_schedule.run(&mut self.world);

        // Send updates to all players.
        let snapshot = {
            let mut w = Vec::new();
            write_snapshot(&mut w, &mut self.world).unwrap();
            w
        };
        for player in self.players.iter().flatten() {
            send_game_data(
                &self.connection_requests,
                player.addr,
                Packet::GameState(GameStatePacket {
                    tick_id: self.current_tick,
                    serialized_game_state: snapshot.clone(),
                }),
            )
            .await;
        }

        self.tick_sleep
            .as_mut()
            .reset(self.current_tick.next().goal_time().to_instant(self.epoch));
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

struct SpawnContext<'a> {
    world: &'a mut World,
    colliders: &'a mut ColliderSet,
    collider_cache: &'a mut ColliderCache,
}

impl<'a> SpawnContext<'a> {
    fn spawn_static_model(&mut self, name: &str, transform: Isometry3<f32>) {
        self.world
            .spawn()
            .insert(Transform(transform))
            .insert(ModelName(name.to_string()));
        self.colliders.insert(
            self.collider_cache
                .get(BorrowedColliderCacheKey::TriangleMesh(&format!(
                    "{name}_col"
                )))
                .position(transform),
        );
    }
}
