use std::collections::{BTreeMap, HashMap};
use std::f32::consts::PI;
use std::mem::{replace, take};
use std::num::NonZeroU8;
use std::sync::Mutex;
use std::time::Duration;

use bevy_ecs::prelude::*;
use dungeon_vr_session_shared::action::{apply_actions, Action};
use dungeon_vr_session_shared::collider_cache::ColliderCache;
use dungeon_vr_session_shared::core::{
    Authority, LocalAuthorityResource, NetId, SynchronizedComponent, TransformComponent,
};
use dungeon_vr_session_shared::fly_around::fly_around;
use dungeon_vr_session_shared::interaction::{GrabbableComponent, HandComponent, HandGrabState};
use dungeon_vr_session_shared::physics::{
    reset_forces, step_physics, sync_physics, update_rigid_body_transforms, PhysicsComponent,
    PhysicsResource,
};
use dungeon_vr_session_shared::render::{ModelHandle, RenderComponent};
use dungeon_vr_session_shared::resources::{AllActionsResource, EntitiesByNetIdResource};
use dungeon_vr_session_shared::snapshot::apply_snapshot;
use dungeon_vr_session_shared::time::{
    ClientEpoch, ClientOffset, ClientTimeToServerTime, LocalTime, TimeOffset,
};
use dungeon_vr_session_shared::{PlayerId, TickId, TICK_INTERVAL};
use itertools::{merge_join_by, EitherOrBoth};
use ordered_float::NotNan;
use rapier3d::na::{zero, Matrix4, Vector4};
use rapier3d::prelude::*;
use slotmap::SecondaryMap;

use crate::asset::{MaterialAssets, ModelAssets};
use crate::render_data::RenderData;
use crate::vk_handles::VkHandles;

#[derive(Clone, Copy)]
pub struct XrMarker;

type XrTime = LocalTime<XrMarker>;
type XrOffset = TimeOffset<XrMarker, XrMarker>;

struct VrTrackingState {
    current: VrTracking,
    prev: VrTracking,
}

#[derive(Clone, Copy, Default)]
pub struct VrTracking {
    pub view: openxr::Posef,
    pub hands: [VrHand; 2],
}

#[derive(Clone, Copy, Default)]
pub struct VrHand {
    pub pose: Isometry<f32>,
    pub squeeze: f32,
    pub squeeze_force: f32,
}

pub struct Game {
    ecs: GameEcs,
    net: Option<GameNet>,
    tick: GameTick,
    render: Mutex<GameRender>,
    prev_vr_tracking: VrTracking,
}

struct GameEcs {
    world: World,
    local_actions_schedule: Schedule,
    apply_actions_schedule: Schedule,
    core_update_schedule: Schedule,
    update_schedule: Schedule,
}

struct GameNet {
    local_player_id: PlayerId,
    latest: Option<AuthoritativeState>,
    local_actions: BTreeMap<TickId, Vec<Action>>,
    action_accumulator: Vec<Action>,
    time_sync: Option<TimeSync>,
}

struct AuthoritativeState {
    tick_id: TickId,
    snapshot: Vec<u8>,
}

#[derive(Clone, Copy)]
struct TimeSync {
    client_epoch: ClientEpoch,
    round_trip_time: ClientOffset,
    offset: ClientTimeToServerTime,
}

struct GameTick {
    /// The ID of the most recently completed tick.
    last_completed_tick_id: TickId,
    /// When the next tick is scheduled to occur, or `None` before the first tick to indicate it
    /// should occur as soon as possible.
    next_tick_time: Option<XrTime>,
    /// The current tick interval, which is nominally [`TICK_INTERVAL`], but varies under server
    /// control to maintain a desired action buffer size.
    tick_interval: XrOffset,
}

struct GameRender {
    start_time: XrTime,
    tick_interval: XrOffset,
    snapshots: [RenderSnapshot; 2],
}

#[derive(Default)]
struct RenderSnapshot {
    /// Sorted by `entity_id`.
    model_transforms: Vec<RenderEntity>,
}

struct RenderEntity {
    entity_id: u32,
    model_handle: ModelHandle,
    transform: Isometry<f32>,
    color: Vector4<f32>,
}

impl GameRender {
    /// Posts a render snapshot. This removes the older render snapshot, moves the newer one to the
    /// older slot, and sets the provided snapshot as the newer one. The time span for interpolation
    /// is set based on the current time and the provided tick interval.
    ///
    /// If a post is early, the interpolation parameter will jump from some value less than one to
    /// zero and the player may see a discontinuity where time is lost. If a post is late, the
    /// interpolation parameter will saturate at one and the player may see time stop.
    fn post_at(&mut self, start_time: XrTime, tick_interval: XrOffset, snapshot: RenderSnapshot) {
        self.start_time = start_time;
        self.tick_interval = tick_interval;
        self.snapshots[0] = replace(&mut self.snapshots[1], snapshot);
    }

    fn interpolate_at(
        &self,
        time: XrTime,
    ) -> SecondaryMap<ModelHandle, Vec<(Matrix4<f32>, Vector4<f32>)>> {
        let t = ((time - self.start_time).to_nanos() as f32 / self.tick_interval.to_nanos() as f32)
            .clamp(0.0, 1.0);

        let mut result: SecondaryMap<_, Vec<_>> = Default::default();
        for items in merge_join_by(
            &self.snapshots[0].model_transforms,
            &self.snapshots[1].model_transforms,
            |a, b| a.entity_id.cmp(&b.entity_id),
        ) {
            let (model_handle, transform, color) = match items {
                EitherOrBoth::Both(a, b) => (
                    b.model_handle,
                    Isometry::try_lerp_slerp(&a.transform, &b.transform, t, 0.0)
                        .unwrap_or(b.transform),
                    a.color.lerp(&b.color, t),
                ),
                EitherOrBoth::Left(e) => (e.model_handle, e.transform, e.color),
                EitherOrBoth::Right(e) => (e.model_handle, e.transform, e.color),
            };
            if let Some(entry) = result.entry(model_handle) {
                entry.or_default().push((transform.to_matrix(), color));
            }
        }
        result
    }
}

#[derive(Default)]
struct LocalActionsResource(Vec<Action>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, StageLabel)]
enum StageLabel {
    Singleton,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SystemLabel)]
enum SystemLabel {
    Init,
    UpdateBeforePhysics,
    PhysicsStep,
    UpdateAfterPhysics,
    Render,
}

pub struct UpdateResult {
    pub model_transform_colors: SecondaryMap<ModelHandle, Vec<(Matrix4<f32>, Vector4<f32>)>>,
    pub actions_committed: BTreeMap<TickId, Vec<Action>>,
    pub owned_transforms: HashMap<NetId, Isometry<f32>>,
}

impl Game {
    pub fn new() -> Self {
        let mut world = World::new();
        let bodies = RigidBodySet::new();
        let colliders = ColliderSet::new();
        let collider_cache = ColliderCache::new();

        // Spawn local hands. These will be replaced if joining an online session.
        for index in 0..2 {
            world
                .spawn()
                .insert(TransformComponent::default())
                .insert(RenderComponent::new(["left_hand", "right_hand"][index]))
                .insert(HandComponent {
                    index,
                    grab_state: HandGrabState::Empty,
                });
        }

        let local_actions_schedule = Schedule::default().with_stage(
            StageLabel::Singleton,
            SystemStage::parallel().with_system(emit_hand_actions),
        );

        let apply_actions_schedule = Schedule::default()
            .with_stage(StageLabel::Singleton, SystemStage::single(apply_actions));

        let core_update_schedule = Schedule::default().with_stage(
            StageLabel::Singleton,
            SystemStage::parallel()
                .with_system(apply_actions)
                .with_system(fly_around),
        );

        let update_schedule = Schedule::default().with_stage(
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
                        .label(SystemLabel::UpdateBeforePhysics)
                        .with_system(update_hands),
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
                )
                .with_system_set(
                    SystemSet::new()
                        .after(SystemLabel::UpdateAfterPhysics)
                        .label(SystemLabel::Render)
                        .with_system(gather_local_model_transforms)
                        .with_system(gather_owned_transforms),
                ),
        );

        world.insert_resource(PhysicsResource::new(
            bodies,
            colliders,
            collider_cache,
            1.0 / 120.0,
        ));
        world.insert_resource(EntitiesByNetIdResource::default());
        world.insert_resource(LocalAuthorityResource(None));
        world.insert_resource(LocalModelTransformColors::default());
        world.insert_resource(OwnedTransforms::default());

        Self {
            ecs: GameEcs {
                world,
                local_actions_schedule,
                apply_actions_schedule,
                core_update_schedule,
                update_schedule,
            },
            net: None,
            tick: GameTick {
                last_completed_tick_id: TickId(0),
                next_tick_time: None,
                tick_interval: XrOffset::from_nanos(TICK_INTERVAL.as_nanos() as i64),
            },
            render: Mutex::new(GameRender {
                start_time: XrTime::from_nanos_since_epoch(0),
                tick_interval: XrOffset::from_nanos(1),
                snapshots: Default::default(),
            }),
            prev_vr_tracking: VrTracking::default(),
        }
    }

    pub fn start_net_session(&mut self, local_player_id: PlayerId) {
        log::info!("Starting net session");
        assert!(self.net.is_none());
        self.net = Some(GameNet {
            local_player_id,
            latest: None,
            local_actions: BTreeMap::default(),
            action_accumulator: Vec::new(),
            time_sync: None,
        });
        self.ecs.world.resource_mut::<LocalAuthorityResource>().0 =
            Some(Authority::Player(local_player_id));

        // Despawn any unsynchronized hands.
        let unsynchronized_hands = Vec::from_iter(
            self.ecs
                .world
                .query_filtered::<Entity, (With<HandComponent>, Without<SynchronizedComponent>)>()
                .iter(&self.ecs.world),
        );
        for entity in unsynchronized_hands {
            self.ecs.world.despawn(entity);
        }
    }

    pub fn handle_snapshot(
        &mut self,
        snapshot_tick_id: TickId,
        tick_interval: Duration,
        snapshot_data: Vec<u8>,
    ) {
        let net = self.net.as_mut().unwrap();

        // Reject snapshots that don't advance time.
        if matches!(net.latest, Some(AuthoritativeState { tick_id, .. }) if snapshot_tick_id <= tick_id)
        {
            return;
        }

        // Accept the server's tick interval assignment.
        self.tick.tick_interval =
            XrOffset::from_nanos(tick_interval.as_nanos().try_into().unwrap());

        // Go directly to the new snapshot.
        let mut r = snapshot_data.as_slice();
        apply_snapshot(&mut r, &mut self.ecs.world).unwrap();
        assert!(r.is_empty());
        net.latest = Some(AuthoritativeState {
            tick_id: snapshot_tick_id,
            snapshot: snapshot_data,
        });
        let goal_tick_id = replace(&mut self.tick.last_completed_tick_id, snapshot_tick_id);

        // Discard obsolete local actions.
        net.local_actions
            .retain(|&action_tick_id, _| action_tick_id > snapshot_tick_id);

        // Simulate forward to the previous last-completed tick.
        while self.tick.last_completed_tick_id < goal_tick_id {
            let this_tick_id = self.tick.last_completed_tick_id.next();

            let local_actions = match net.local_actions.get(&this_tick_id) {
                Some(actions) => actions.clone(),
                None => vec![],
            };
            let all_actions =
                AllActionsResource([(net.local_player_id, local_actions)].into_iter().collect());
            self.ecs.tick(all_actions);

            self.tick.last_completed_tick_id = this_tick_id;
        }

        // The simulation is now back to where it was, but corrected for any known deviations.
    }

    pub fn set_vr_tracking(&mut self, vr_tracking: VrTracking) {
        self.ecs.world.insert_resource(VrTrackingState {
            current: vr_tracking,
            prev: self.prev_vr_tracking,
        });
        self.prev_vr_tracking = vr_tracking;
    }

    pub fn update(
        &mut self,
        vk: &VkHandles,
        render: &RenderData,
        material_assets: &mut MaterialAssets,
        model_assets: &mut ModelAssets,
        predicted_display_time: openxr::Time,
    ) -> UpdateResult {
        let predicted_display_time =
            XrTime::from_nanos_since_epoch(predicted_display_time.as_nanos().try_into().unwrap());

        // Capture local actions and predict their effects.
        let local_actions = self.ecs.get_local_actions();
        if let Some(net) = self.net.as_mut() {
            net.action_accumulator.extend(local_actions.iter().copied());
        }
        let local_player_id = self
            .net
            .as_mut()
            .map(|net| net.local_player_id)
            // TODO: Decide what to do about local player renumbering in online/offline transitions.
            .unwrap_or(PlayerId(NonZeroU8::new(1).unwrap()));
        self.ecs.apply_actions(local_player_id, local_actions);

        // Tick up to the current time.
        let mut actions_committed = BTreeMap::new();
        let mut ticked = false;
        while self
            .tick
            .next_tick_time
            .map(|time| time <= predicted_display_time)
            .unwrap_or(true)
        {
            let this_tick_time = self.tick.next_tick_time.unwrap_or(predicted_display_time);
            let this_tick_id = self.tick.last_completed_tick_id.next();

            // Commit this tick's actions.
            if let Some(net) = self.net.as_mut() {
                let actions = take(&mut net.action_accumulator);
                net.local_actions.insert(this_tick_id, actions.clone());
                actions_committed.insert(this_tick_id, actions);
            }
            // There are no actions to apply. Local actions have already been applied and remote
            // actions haven't arrived yet.
            self.ecs.tick(AllActionsResource::default());
            self.render.lock().unwrap().post_at(
                this_tick_time,
                self.tick.tick_interval,
                self.ecs.take_render_snapshot(),
            );
            ticked = true;

            self.tick.last_completed_tick_id = this_tick_id;
            self.tick.next_tick_time = Some(this_tick_time + self.tick.tick_interval);
        }

        // Finally, perform a fine detail update for physics and rendering.
        self.ecs.update_schedule.run(&mut self.ecs.world);

        self.ecs
            .load_models(vk, render, material_assets, model_assets);
        let mut model_transform_colors = self
            .render
            .lock()
            .unwrap()
            .interpolate_at(predicted_display_time);
        // Merge in locally owned model transforms.
        // TODO: Exclude locally owned objects from the snapshots!
        for (model_handle, transform_colors) in
            take(&mut self.ecs.world.resource_mut::<LocalModelTransformColors>().0)
        {
            if let Some(entry) = model_transform_colors.entry(model_handle) {
                entry.or_default().extend_from_slice(&transform_colors);
            }
        }
        let mut owned_transforms = take(&mut self.ecs.world.resource_mut::<OwnedTransforms>().0);
        if !ticked {
            owned_transforms.clear();
        }
        UpdateResult {
            model_transform_colors,
            actions_committed,
            owned_transforms,
        }
    }

    pub fn handle_time_sync(
        &mut self,
        client_epoch: ClientEpoch,
        round_trip_time: ClientOffset,
        offset: ClientTimeToServerTime,
    ) {
        self.net.as_mut().unwrap().time_sync = Some(TimeSync {
            client_epoch,
            round_trip_time,
            offset,
        });
    }
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl GameEcs {
    fn get_local_actions(&mut self) -> Vec<Action> {
        self.world.insert_resource(LocalActionsResource::default());

        self.local_actions_schedule.run(&mut self.world);

        take(&mut self.world.resource_mut::<LocalActionsResource>().0)
    }

    fn apply_actions(&mut self, local_player_id: PlayerId, local_actions: Vec<Action>) {
        self.world.insert_resource(AllActionsResource(
            [(local_player_id, local_actions)].into_iter().collect(),
        ));

        self.apply_actions_schedule.run(&mut self.world);

        self.world.resource_mut::<AllActionsResource>().0.clear();
    }

    /// Apply actions and advance time.
    fn tick(&mut self, all_actions: AllActionsResource) {
        self.world.insert_resource(all_actions);

        self.core_update_schedule.run(&mut self.world);

        self.world.resource_mut::<AllActionsResource>().0.clear();
    }

    fn take_render_snapshot(&mut self) -> RenderSnapshot {
        let mut model_transforms = Vec::new();
        self.world
            .resource_scope(|world, local_authority: Mut<LocalAuthorityResource>| {
                for (
                    entity,
                    synchronized,
                    TransformComponent(transform),
                    &RenderComponent { model_handle, .. },
                ) in world
                    .query::<(
                        Entity,
                        Option<&SynchronizedComponent>,
                        &TransformComponent,
                        &RenderComponent,
                    )>()
                    .iter(world)
                {
                    if !local_authority.is_local(synchronized) {
                        model_transforms.push(RenderEntity {
                            entity_id: entity.id(),
                            model_handle,
                            transform: *transform,
                            color: synchronized
                                .map(|synchronized| synchronized.authority.to_color())
                                .unwrap_or(Vector4::new(0.0, 1.0, 1.0, 1.0)),
                        });
                    }
                }
            });
        model_transforms.sort_unstable_by_key(|e| e.entity_id);
        RenderSnapshot { model_transforms }
    }

    fn load_models(
        &mut self,
        vk: &VkHandles,
        render: &RenderData,
        material_assets: &mut MaterialAssets,
        model_assets: &mut ModelAssets,
    ) {
        for mut model in self
            .world
            .query::<&mut RenderComponent>()
            .iter_mut(&mut self.world)
        {
            model.model_handle = model_assets.load(vk, render, material_assets, &model.model_name);
        }
    }
}

fn emit_hand_actions(
    query: Query<(&TransformComponent, &HandComponent), Without<GrabbableComponent>>,
    vr_tracking: Res<VrTrackingState>,
    grabbable_query: Query<(
        &SynchronizedComponent,
        &TransformComponent,
        &GrabbableComponent,
    )>,
    mut local_actions: ResMut<LocalActionsResource>,
) {
    for (transform, hand) in query.iter() {
        let vr_hand = &vr_tracking.current.hands[hand.index];
        let prev_vr_hand = &vr_tracking.prev.hands[hand.index];

        // Determine whether to start or end a grab.
        match hand.grab_state {
            HandGrabState::Empty => {
                if vr_hand.squeeze_force > 0.2 && prev_vr_hand.squeeze_force <= 0.2 {
                    if let Some((_dist, net_id)) = grabbable_query
                        .iter()
                        .filter_map(|(synchronized, grabbable_transform, grabbable)| {
                            if grabbable.grabbed {
                                return None;
                            }
                            let dist = (grabbable_transform.0.translation.vector
                                - transform.0.translation.vector)
                                .magnitude();
                            if dist <= 0.1 {
                                Some((dist, synchronized.net_id))
                            } else {
                                None
                            }
                        })
                        .min_by_key(|(dist, _)| NotNan::new(*dist).unwrap())
                    {
                        local_actions.0.push(Action::Grab {
                            hand_index: hand.index,
                            target: net_id,
                        });
                    }
                }
            }
            HandGrabState::Grabbing(_) => {
                if vr_hand.squeeze < 0.8 {
                    local_actions.0.push(Action::Drop {
                        hand_index: hand.index,
                    });
                }
            }
        }
    }
}

fn update_hands(
    mut query: Query<
        (
            Option<&SynchronizedComponent>,
            &mut TransformComponent,
            &HandComponent,
        ),
        Without<GrabbableComponent>,
    >,
    vr_tracking: Res<VrTrackingState>,
    local_authority: Res<LocalAuthorityResource>,
    entities_by_net_id: Res<EntitiesByNetIdResource>,
    mut physics: ResMut<PhysicsResource>,
    grabbable_query: Query<&PhysicsComponent, With<GrabbableComponent>>,
) {
    for (synchronized, mut transform, hand) in query.iter_mut() {
        if !local_authority.is_local(synchronized) {
            continue;
        }

        let vr_hand = &vr_tracking.current.hands[hand.index];
        transform.0 = vr_hand.pose
            * Isometry::from_parts(
                Translation::default(),
                Rotation::from_scaled_axis(vector![25.0 * PI / 180.0, 0.0, 0.0]),
            );

        if let HandGrabState::Grabbing(net_id) = hand.grab_state {
            if let Some(handle) = grabbable_query
                .get(entities_by_net_id.0[&net_id])
                .unwrap()
                .rigid_body
            {
                let inv_dt = 1.0 / physics.integration_parameters.dt;
                let rigid_body = &mut physics.bodies[handle];

                let goal_pos = transform.0.translation.vector;
                let pos_correction = goal_pos - rigid_body.position().translation.vector;
                let one_step_vel = pos_correction * inv_dt;
                rigid_body.set_linvel(one_step_vel, true);

                let goal_rot = transform.0.rotation;
                let rot_correction = goal_rot * rigid_body.rotation().inverse();
                let one_step_angvel = match rot_correction.axis_angle() {
                    Some((axis, angle)) => (angle * inv_dt) * axis.into_inner(),
                    None => zero(),
                };
                rigid_body.set_angvel(one_step_angvel, true);
            }
        }
    }
}

#[derive(Default)]
struct LocalModelTransformColors(SecondaryMap<ModelHandle, Vec<(Matrix4<f32>, Vector4<f32>)>>);

impl LocalModelTransformColors {
    fn insert(
        &mut self,
        model: &RenderComponent,
        transform: &TransformComponent,
        color: Vector4<f32>,
    ) {
        if let Some(entry) = self.0.entry(model.model_handle) {
            entry.or_default().push((transform.0.to_matrix(), color));
        }
    }
}

fn gather_local_model_transforms(
    query: Query<(
        Option<&SynchronizedComponent>,
        &TransformComponent,
        &RenderComponent,
    )>,
    local_authority: Res<LocalAuthorityResource>,
    mut model_transforms: ResMut<LocalModelTransformColors>,
) {
    for (synchronized, transform, model) in query.iter() {
        if local_authority.is_local(synchronized) {
            model_transforms.insert(
                model,
                transform,
                synchronized
                    .map(|synchronized| synchronized.authority.to_color())
                    .unwrap_or(Vector4::new(0.0, 1.0, 1.0, 1.0)),
            );
        }
    }
}

#[derive(Default)]
struct OwnedTransforms(HashMap<NetId, Isometry<f32>>);

fn gather_owned_transforms(
    query: Query<(&SynchronizedComponent, &TransformComponent)>,
    local_authority: Res<LocalAuthorityResource>,
    mut owned_transforms: ResMut<OwnedTransforms>,
) {
    for (synchronized, transform) in query.iter() {
        if local_authority.is_local(Some(synchronized)) {
            owned_transforms.0.insert(synchronized.net_id, transform.0);
        }
    }
}
