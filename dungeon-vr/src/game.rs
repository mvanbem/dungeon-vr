use std::collections::{btree_map, BTreeMap};
use std::f32::consts::PI;
use std::mem::take;
use std::num::NonZeroU8;

use bevy_ecs::prelude::*;
use dungeon_vr_session_shared::action::{apply_actions, Action};
use dungeon_vr_session_shared::collider_cache::ColliderCache;
use dungeon_vr_session_shared::core::{Synchronized, TransformComponent};
use dungeon_vr_session_shared::fly_around::fly_around;
use dungeon_vr_session_shared::interaction::{GrabbableComponent, HandComponent, HandGrabState};
use dungeon_vr_session_shared::physics::{sync_physics, PhysicsComponent, PhysicsResource};
use dungeon_vr_session_shared::render::{ModelHandle, RenderComponent};
use dungeon_vr_session_shared::resources::{AllActions, EntitiesByNetId};
use dungeon_vr_session_shared::snapshot::apply_snapshot;
use dungeon_vr_session_shared::time::{ClientEpoch, ClientOffset, ClientTimeToServerTime};
use dungeon_vr_session_shared::{PlayerId, TickId};
use ordered_float::NotNan;
use rapier3d::na::{self, Isometry3, Matrix4, Translation, UnitQuaternion};
use rapier3d::prelude::*;
use slotmap::SecondaryMap;

use crate::asset::{MaterialAssets, ModelAssets};
use crate::render_data::RenderData;
use crate::vk_handles::VkHandles;

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
    pub pose: Isometry3<f32>,
    pub squeeze: f32,
    pub squeeze_force: f32,
}

pub struct Game {
    ecs: GameEcs,
    net: GameNet,
    current_tick: TickId,
    prev_vr_tracking: VrTracking,
}

struct GameEcs {
    world: World,
    local_actions_schedule: Schedule,
    core_update_schedule: Schedule,
    update_schedule: Schedule,
}

struct GameNet {
    local_player_id: PlayerId,
    latest: Option<AuthoritativeState>,
    local_actions: BTreeMap<TickId, Vec<Action>>,
    time_sync: Option<TimeSync>,
}

#[derive(Clone, Copy)]
struct TimeSync {
    client_epoch: ClientEpoch,
    round_trip_time: ClientOffset,
    offset: ClientTimeToServerTime,
}

struct AuthoritativeState {
    tick_id: TickId,
    snapshot: Vec<u8>,
}

#[derive(Default)]
struct LocalActions(Vec<Action>);

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

impl Game {
    pub fn new() -> Self {
        let mut world = World::new();
        let bodies = RigidBodySet::new();
        let colliders = ColliderSet::new();
        let collider_cache = ColliderCache::new();

        // TODO: The server will need to spawn these in order to assign net IDs.
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
                        .with_system(physics_step),
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
                        .with_system(gather_models),
                ),
        );

        world.insert_resource(PhysicsResource::new(
            bodies,
            colliders,
            collider_cache,
            1.0 / 120.0,
        ));
        world.insert_resource(EntitiesByNetId::default());
        world.insert_resource(ModelTransforms::default());

        Self {
            ecs: GameEcs {
                world,
                local_actions_schedule,
                core_update_schedule,
                update_schedule,
            },
            net: GameNet {
                local_player_id: PlayerId(NonZeroU8::new(1).unwrap()), // TODO: This is wrong!
                latest: None,
                local_actions: BTreeMap::default(),
                time_sync: None,
            },
            current_tick: TickId(0),
            prev_vr_tracking: VrTracking::default(),
        }
    }

    pub fn handle_snapshot(&mut self, snapshot_tick_id: TickId, snapshot_data: Vec<u8>) {
        // Reject snapshots that don't advance time.
        if matches!(self.net.latest, Some(AuthoritativeState { tick_id, .. }) if snapshot_tick_id <= tick_id)
        {
            return;
        }

        // Go directly to the new snapshot.
        let mut r = snapshot_data.as_slice();
        apply_snapshot(&mut r, &mut self.ecs.world).unwrap();
        assert!(r.is_empty());
        self.net.latest = Some(AuthoritativeState {
            tick_id: snapshot_tick_id,
            snapshot: snapshot_data,
        });
        self.current_tick = snapshot_tick_id;

        // Discard obsolete local actions.
        self.net
            .local_actions
            .retain(|&action_tick_id, _| action_tick_id > snapshot_tick_id);

        // NOTE: This almost certainly leaves the game rewound a ways into the past. This will be
        // corrected in the next call to update().
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
    ) -> SecondaryMap<ModelHandle, Vec<Matrix4<f32>>> {
        self.ecs
            .load_models(vk, render, material_assets, model_assets);

        // Perform core ticks to try to catch up to the goal tick.
        if let Some(time_sync) = self.net.time_sync {
            let server_now = time_sync.client_epoch.now() + time_sync.offset;
            let goal_tick_id = TickId::latest_tick_for_time(server_now);

            let mut core_ticks_performed = 0;
            while self.current_tick < goal_tick_id && core_ticks_performed < 100 {
                self.current_tick = self.current_tick.next();

                let local_actions = match self.net.local_actions.entry(self.current_tick) {
                    // Reuse any recorded local actions for this tick.
                    btree_map::Entry::Occupied(entry) => entry.get().clone(),
                    // If fully caught up, capture local actions and save them.
                    btree_map::Entry::Vacant(entry) if self.current_tick == goal_tick_id => {
                        let local_actions = self.ecs.get_local_actions();
                        entry.insert(local_actions.clone());
                        local_actions
                    }
                    // Otherwise, leave history unchanged and take no local actions for this tick.
                    btree_map::Entry::Vacant(_) => Vec::new(),
                };
                let all_actions = AllActions(
                    [(self.net.local_player_id, local_actions)]
                        .into_iter()
                        .collect(),
                );

                self.ecs.core_update(all_actions);
                core_ticks_performed += 1;
            }
        }

        // Run the non-core update.
        self.ecs.update_schedule.run(&mut self.ecs.world);

        take(&mut self.ecs.world.resource_mut::<ModelTransforms>().0)
    }

    pub fn handle_time_sync(
        &mut self,
        client_epoch: ClientEpoch,
        round_trip_time: ClientOffset,
        offset: ClientTimeToServerTime,
    ) {
        self.net.time_sync = Some(TimeSync {
            client_epoch,
            round_trip_time,
            offset,
        });
    }
}

impl GameEcs {
    fn get_local_actions(&mut self) -> Vec<Action> {
        self.world.insert_resource(LocalActions::default());

        self.local_actions_schedule.run(&mut self.world);

        take(&mut self.world.resource_mut::<LocalActions>().0)
    }

    /// The subset of the update for client/server shared behavior.
    fn core_update(&mut self, all_actions: AllActions) {
        self.world.insert_resource(all_actions);

        self.core_update_schedule.run(&mut self.world);

        *self.world.resource_mut() = AllActions::default();
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
            // TODO: Only if changed?
            model.model_handle = model_assets.load(vk, render, material_assets, &model.model_name);
        }
    }
}

fn reset_forces(mut physics: ResMut<PhysicsResource>) {
    for (_, body) in physics.bodies.iter_mut() {
        body.reset_forces(false);
    }
}

fn emit_hand_actions(
    query: Query<(&TransformComponent, &HandComponent), Without<GrabbableComponent>>,
    vr_tracking: Res<VrTrackingState>,
    grabbable_query: Query<(&Synchronized, &TransformComponent, &GrabbableComponent)>,
    mut local_actions: ResMut<LocalActions>,
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
    mut query: Query<(&mut TransformComponent, &HandComponent), Without<GrabbableComponent>>,
    vr_tracking: Res<VrTrackingState>,
    entities_by_net_id: Res<EntitiesByNetId>,
    mut physics: ResMut<PhysicsResource>,
    grabbable_query: Query<&PhysicsComponent, With<GrabbableComponent>>,
) {
    for (mut transform, hand) in query.iter_mut() {
        let vr_hand = &vr_tracking.current.hands[hand.index];
        transform.0 = vr_hand.pose
            * Isometry3::from_parts(
                Translation::default(),
                UnitQuaternion::from_scaled_axis(vector![25.0 * PI / 180.0, 0.0, 0.0]),
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
                    None => na::zero(),
                };
                rigid_body.set_angvel(one_step_angvel, true);
            }
        }
    }
}

fn physics_step(mut physics: ResMut<PhysicsResource>) {
    let PhysicsResource {
        bodies,
        colliders,
        integration_parameters,
        physics_pipeline,
        islands,
        broad_phase,
        narrow_phase,
        impulse_joints,
        multibody_joints,
        ccd_solver,
        ..
    } = &mut *physics;
    physics_pipeline.step(
        &(vector![0.0, -9.81, 0.0]),
        &integration_parameters,
        islands,
        broad_phase,
        narrow_phase,
        bodies,
        colliders,
        impulse_joints,
        multibody_joints,
        ccd_solver,
        &(),
        &(),
    );
}

fn update_rigid_body_transforms(
    mut query: Query<(&mut TransformComponent, &PhysicsComponent)>,
    game_physics: Res<PhysicsResource>,
) {
    for (mut transform, physics) in query.iter_mut() {
        if let Some(handle) = physics.rigid_body {
            let body = &game_physics.bodies[handle];
            transform.0 = Isometry3::from_parts((*body.translation()).into(), *body.rotation());
        }
    }
}

#[derive(Default)]
struct ModelTransforms(SecondaryMap<ModelHandle, Vec<Matrix4<f32>>>);

impl ModelTransforms {
    fn insert(&mut self, model: &RenderComponent, transform: &TransformComponent) {
        self.0
            .entry(model.model_handle)
            .unwrap()
            .or_default()
            .push(transform.0.to_matrix());
    }
}

fn gather_models(
    query: Query<(&TransformComponent, &RenderComponent)>,
    mut model_transforms: ResMut<ModelTransforms>,
) {
    for (transform, model) in query.iter() {
        model_transforms.insert(model, transform);
    }
}
