use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, PI};
use std::mem::replace;

use bevy_ecs::prelude::*;
use dungeon_vr_session_shared::net_game::{apply_snapshot, ModelName, NetId, Transform};
use ordered_float::NotNan;
use rapier3d::na::{self, Isometry3, Matrix4, Translation, UnitQuaternion};
use rapier3d::prelude::*;
use slotmap::SecondaryMap;

use crate::asset::{MaterialAssets, ModelAssetKey, ModelAssets};
use crate::collider_cache::{BorrowedColliderCacheKey, ColliderCache};
use crate::render_data::RenderData;
use crate::vk_handles::VkHandles;

#[derive(Component)]
struct ModelRenderer {
    model_key: ModelAssetKey,
}

#[derive(Component)]
struct Hand {
    index: usize,
    grab_state: HandGrabState,
}

enum HandGrabState {
    Empty,
    Grabbing(Entity),
}

#[derive(Component)]
struct Grabbable {
    grabbed: bool,
}

#[derive(Component)]
struct RigidBody {
    handle: RigidBodyHandle,
}

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
    world: World,
    schedule: Schedule,
    prev_vr_tracking: VrTracking,
}

struct GamePhysics {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    integration_parameters: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: BroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
}

impl Game {
    pub fn new(
        vk: &VkHandles,
        render: &RenderData,
        material_assets: &mut MaterialAssets,
        model_assets: &mut ModelAssets,
    ) -> Self {
        let mut world = World::new();
        let mut bodies = RigidBodySet::new();
        let mut colliders = ColliderSet::new();
        let mut collider_cache = ColliderCache::new();

        for index in 0..2 {
            world
                .spawn()
                .insert(Transform::default())
                .insert(ModelRenderer {
                    model_key: model_assets.load(
                        vk,
                        render,
                        material_assets,
                        ["left_hand", "right_hand"][index],
                    ),
                })
                .insert(Hand {
                    index,
                    grab_state: HandGrabState::Empty,
                });
        }

        let mut spawn_context = SpawnContext {
            vk,
            render,
            material_assets,
            model_assets,
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

        let key_body = bodies.insert(
            RigidBodyBuilder::dynamic()
                .translation(vector![0.0, 1.0, 0.0])
                .ccd_enabled(true)
                .build(),
        );
        colliders.insert_with_parent(
            collider_cache.get(BorrowedColliderCacheKey::ConvexHull(
                "LowPolyDungeon/Key_Silver",
            )),
            key_body,
            &mut bodies,
        );
        world
            .spawn()
            .insert(Transform(vector![0.0, 1.0, 0.0].into()))
            .insert(ModelRenderer {
                model_key: model_assets.load(
                    vk,
                    render,
                    material_assets,
                    "LowPolyDungeon/Key_Silver",
                ),
            })
            .insert(Grabbable { grabbed: false })
            .insert(RigidBody { handle: key_body });

        let schedule = Schedule::default()
            .with_stage(
                "reset_forces",
                SystemStage::parallel().with_system(reset_forces),
            )
            .with_stage_after(
                "reset_forces",
                "pre_step",
                SystemStage::parallel().with_system(update_hands),
            )
            .with_stage_after(
                "pre_step",
                "physics_step",
                SystemStage::parallel().with_system(physics_step),
            )
            .with_stage_after(
                "physics_step",
                "post_step",
                SystemStage::parallel().with_system(update_rigid_body_transforms),
            )
            .with_stage_after(
                "post_step",
                "render",
                SystemStage::parallel().with_system(gather_models),
            );

        world.insert_resource(GamePhysics {
            bodies,
            colliders,
            integration_parameters: IntegrationParameters {
                dt: 1.0 / 120.0,
                ..Default::default()
            },
            physics_pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
        });
        world.insert_resource(BTreeMap::<NetId, Entity>::default());
        world.insert_resource(ModelTransforms::default());

        Self {
            world,
            schedule,
            prev_vr_tracking: Default::default(),
        }
    }

    pub fn handle_snapshot(&mut self, snapshot: Vec<u8>) {
        let mut r = snapshot.as_slice();
        apply_snapshot(&mut r, &mut self.world).unwrap();
        assert!(r.is_empty());
    }

    pub fn update(
        &mut self,
        vk: &VkHandles,
        render: &RenderData,
        material_assets: &mut MaterialAssets,
        model_assets: &mut ModelAssets,
        vr_tracking: VrTracking,
    ) -> SecondaryMap<ModelAssetKey, Vec<Matrix4<f32>>> {
        self.load_models(vk, render, material_assets, model_assets);

        self.world.insert_resource(VrTrackingState {
            current: vr_tracking,
            prev: self.prev_vr_tracking,
        });

        self.schedule.run(&mut self.world);

        self.prev_vr_tracking = vr_tracking;
        self.world.resource_mut::<ModelTransforms>().take()
    }

    fn load_models(
        &mut self,
        vk: &VkHandles,
        render: &RenderData,
        material_assets: &mut MaterialAssets,
        model_assets: &mut ModelAssets,
    ) {
        let mut changes = Vec::new();
        for (entity, model_name) in self
            .world
            .query_filtered::<(Entity, &ModelName), Without<ModelRenderer>>()
            .iter_mut(&mut self.world)
        {
            changes.push((
                entity,
                model_assets.load(vk, render, material_assets, &model_name.0),
            ));
        }

        for (entity, model_key) in changes {
            let mut entity = self.world.entity_mut(entity);
            entity.insert(ModelRenderer { model_key });
        }
    }
}

struct SpawnContext<'a> {
    vk: &'a VkHandles,
    render: &'a RenderData<'a>,
    material_assets: &'a mut MaterialAssets,
    model_assets: &'a mut ModelAssets,
    world: &'a mut World,
    colliders: &'a mut ColliderSet,
    collider_cache: &'a mut ColliderCache,
}

impl<'a> SpawnContext<'a> {
    fn spawn_static_model(&mut self, name: &str, transform: Isometry3<f32>) {
        self.world
            .spawn()
            .insert(Transform(transform))
            .insert(ModelRenderer {
                model_key: self
                    .model_assets
                    .load(self.vk, self.render, self.material_assets, name),
            });
        self.colliders.insert(
            self.collider_cache
                .get(BorrowedColliderCacheKey::TriangleMesh(&format!(
                    "{name}_col"
                )))
                .position(transform),
        );
    }
}

fn reset_forces(mut physics: ResMut<GamePhysics>) {
    for (_, body) in physics.bodies.iter_mut() {
        body.reset_forces(false);
    }
}

fn update_hands(
    mut query: Query<(&mut Transform, &mut Hand), Without<Grabbable>>,
    vr_tracking: Res<VrTrackingState>,
    mut grabbable_query: Query<(Entity, &mut Transform, &mut Grabbable, &RigidBody)>,
    mut physics: ResMut<GamePhysics>,
) {
    for (mut transform, mut hand) in query.iter_mut() {
        let vr_hand = &vr_tracking.current.hands[hand.index];
        let prev_vr_hand = &vr_tracking.prev.hands[hand.index];
        transform.0 = vr_hand.pose
            * Isometry3::from_parts(
                Translation::default(),
                UnitQuaternion::from_scaled_axis(vector![25.0 * PI / 180.0, 0.0, 0.0]),
            );

        // Step the grab state machine.
        match hand.grab_state {
            HandGrabState::Empty => {
                if vr_hand.squeeze_force > 0.2 && prev_vr_hand.squeeze_force <= 0.2 {
                    if let Some((_, entity, _, mut grabbable)) = grabbable_query
                        .iter_mut()
                        .filter_map(|(entity, grabbable_transform, grabbable, _)| {
                            if grabbable.grabbed {
                                return None;
                            }
                            let dist = (grabbable_transform.0.translation.vector
                                - transform.0.translation.vector)
                                .magnitude();
                            if dist <= 0.1 {
                                Some((dist, entity, grabbable_transform, grabbable))
                            } else {
                                None
                            }
                        })
                        .min_by_key(|(dist, _, _, _)| NotNan::new(*dist).unwrap())
                    {
                        // Start grabbing.
                        hand.grab_state = HandGrabState::Grabbing(entity);
                        grabbable.grabbed = true;
                    }
                }
            }
            HandGrabState::Grabbing(entity) => {
                if vr_hand.squeeze < 0.8 {
                    // End grabbing.
                    hand.grab_state = HandGrabState::Empty;
                    grabbable_query
                        .get_component_mut::<Grabbable>(entity)
                        .unwrap()
                        .grabbed = false;
                }
            }
        }

        // Update a held object.
        if let HandGrabState::Grabbing(entity) = hand.grab_state {
            let inv_dt = 1.0 / physics.integration_parameters.dt;
            let rigid_body = &mut physics.bodies[grabbable_query
                .get_component::<RigidBody>(entity)
                .unwrap()
                .handle];

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

fn physics_step(mut physics: ResMut<GamePhysics>) {
    let GamePhysics {
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
    mut query: Query<(&mut Transform, &RigidBody)>,
    physics: Res<GamePhysics>,
) {
    for (mut transform, rigid_body) in query.iter_mut() {
        let body = &physics.bodies[rigid_body.handle];
        transform.0 = Isometry3::from_parts((*body.translation()).into(), *body.rotation());
    }
}

#[derive(Default)]
struct ModelTransforms(SecondaryMap<ModelAssetKey, Vec<Matrix4<f32>>>);

impl ModelTransforms {
    fn insert(&mut self, model_renderer: &ModelRenderer, transform: &Transform) {
        self.0
            .entry(model_renderer.model_key)
            .unwrap()
            .or_default()
            .push(transform.0.to_matrix());
    }

    fn take(&mut self) -> SecondaryMap<ModelAssetKey, Vec<Matrix4<f32>>> {
        replace(&mut self.0, Default::default())
    }
}

fn gather_models(
    query: Query<(&Transform, &ModelRenderer)>,
    mut model_transforms: ResMut<ModelTransforms>,
) {
    for (transform, model_renderer) in query.iter() {
        model_transforms.insert(model_renderer, transform);
    }
}
