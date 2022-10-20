use bevy_ecs::prelude::*;
use rapier3d::prelude::*;

use crate::collider_cache::{BorrowedColliderCacheKey, ColliderCache};
use crate::core::{LocalAuthorityResource, SynchronizedComponent, TransformComponent};
use crate::{NetComponent, NetComponentDestroyContext};

#[derive(Clone, Debug, Component)]
pub struct PhysicsComponent {
    pub collider_name: String,
    pub mode: NetPhysicsMode,
    pub collider: Option<ColliderHandle>,
    pub rigid_body: Option<RigidBodyHandle>,
}

#[derive(Clone, Copy, Debug)]
pub enum NetPhysicsMode {
    Static,
    Dynamic { ccd_enabled: bool },
}

impl PhysicsComponent {
    pub fn new_static(collider_name: impl Into<String>) -> Self {
        Self {
            collider_name: collider_name.into(),
            mode: NetPhysicsMode::Static,
            collider: None,
            rigid_body: None,
        }
    }

    pub fn new_dynamic(collider_name: impl Into<String>) -> Self {
        Self {
            collider_name: collider_name.into(),
            mode: NetPhysicsMode::Dynamic { ccd_enabled: false },
            collider: None,
            rigid_body: None,
        }
    }

    pub fn new_dynamic_ccd(collider_name: impl Into<String>) -> Self {
        Self {
            collider_name: collider_name.into(),
            mode: NetPhysicsMode::Dynamic { ccd_enabled: true },
            collider: None,
            rigid_body: None,
        }
    }
}

impl NetComponent for PhysicsComponent {
    fn apply_snapshot(&mut self, snapshot: Self) {
        self.collider_name = snapshot.collider_name;
        self.mode = snapshot.mode;
    }

    fn destroy(self, ctx: NetComponentDestroyContext) {
        let PhysicsResource {
            bodies,
            colliders,
            islands,
            impulse_joints,
            multibody_joints,
            ..
        } = ctx.physics;
        if let Some(collider) = self.collider {
            colliders.remove(collider, islands, bodies, false);
        }
        if let Some(rigid_body) = self.rigid_body {
            bodies.remove(
                rigid_body,
                islands,
                colliders,
                impulse_joints,
                multibody_joints,
                false,
            );
        }
    }
}

pub struct PhysicsResource {
    pub bodies: RigidBodySet,
    pub colliders: ColliderSet,
    pub collider_cache: ColliderCache,
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub islands: IslandManager,
    pub broad_phase: BroadPhase,
    pub narrow_phase: NarrowPhase,
    pub impulse_joints: ImpulseJointSet,
    pub multibody_joints: MultibodyJointSet,
    pub ccd_solver: CCDSolver,
}

impl PhysicsResource {
    pub fn new(
        bodies: RigidBodySet,
        colliders: ColliderSet,
        collider_cache: ColliderCache,
        dt: f32,
    ) -> Self {
        Self {
            bodies,
            colliders,
            collider_cache,
            integration_parameters: IntegrationParameters {
                dt,
                ..Default::default()
            },
            physics_pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
        }
    }
}

pub fn reset_forces(mut physics: ResMut<PhysicsResource>) {
    for (_, body) in physics.bodies.iter_mut() {
        body.reset_forces(false);
    }
}

pub fn sync_physics(
    mut query: Query<(
        Option<&SynchronizedComponent>,
        &TransformComponent,
        &mut PhysicsComponent,
    )>,
    local_authority: Res<LocalAuthorityResource>,
    mut physics_resource: ResMut<PhysicsResource>,
) {
    // Dereference so that struct fields can be borrowed independently.
    let physics_resource = &mut *physics_resource;

    for (synchronized, transform, mut physics) in query.iter_mut() {
        // Determine the rigid body goal state.
        struct RigidBodyParams {
            body_type: RigidBodyType,
            ccd_enabled: bool,
        }
        let goal = match physics.mode {
            NetPhysicsMode::Dynamic { ccd_enabled } if local_authority.is_local(synchronized) => {
                // Dynamic rigid body.
                Some(RigidBodyParams {
                    body_type: RigidBodyType::Dynamic,
                    ccd_enabled,
                })
            }
            NetPhysicsMode::Dynamic { .. } => {
                // Kinematic rigid body.
                Some(RigidBodyParams {
                    body_type: RigidBodyType::KinematicPositionBased,
                    ccd_enabled: false,
                })
            }
            NetPhysicsMode::Static => {
                // Static collider.
                None
            }
        };

        // Create, update, or destroy the rigid body if necessary.
        match (physics.rigid_body, goal) {
            (Some(handle), Some(goal)) => {
                // Rigid body is present and wanted. Update its attributes.
                let rigid_body = physics_resource.bodies.get_mut(handle).unwrap();
                rigid_body.set_body_type(goal.body_type);
                rigid_body.enable_ccd(goal.ccd_enabled);
                if rigid_body.is_kinematic() {
                    rigid_body.set_next_kinematic_position(transform.0);
                }
            }
            (Some(handle), None) => {
                // Rigid body is present and unwanted. Remove it.
                log::info!("Removing a rigid body");
                physics_resource.bodies.remove(
                    handle,
                    &mut physics_resource.islands,
                    &mut physics_resource.colliders,
                    &mut physics_resource.impulse_joints,
                    &mut physics_resource.multibody_joints,
                    // NOTE: `false` here means to detach the colliders, but keep them around. This
                    // is the right behavior if a dynamic object becomes static.
                    false,
                );
                physics.rigid_body = None;
            }
            (None, Some(goal)) => {
                // Rigid body is not present, but is wanted. Create it.
                log::info!("Creating a rigid body");
                physics.rigid_body = Some(
                    physics_resource.bodies.insert(
                        RigidBodyBuilder::new(goal.body_type)
                            .translation(transform.0.translation.vector)
                            .ccd_enabled(goal.ccd_enabled)
                            .build(),
                    ),
                );
                // If there is already a collider, associate it with the new rigid body.
                if let Some(collider) = physics.collider {
                    log::info!("...and setting it as the existing collider's parent");
                    physics_resource.colliders.set_parent(
                        collider,
                        physics.rigid_body,
                        &mut physics_resource.bodies,
                    );
                }
            }
            (None, None) => (),
        }

        // Create the collider if necessary.
        if physics.collider.is_none() {
            let key = match physics.mode {
                NetPhysicsMode::Static => {
                    BorrowedColliderCacheKey::TriangleMesh(&physics.collider_name)
                }
                NetPhysicsMode::Dynamic { .. } => {
                    BorrowedColliderCacheKey::ConvexHull(&physics.collider_name)
                }
            };
            let collider = physics_resource.collider_cache.get(key);
            match physics.rigid_body {
                Some(rigid_body) => {
                    log::info!("Creating a dynamic collider");
                    physics.collider = Some(physics_resource.colliders.insert_with_parent(
                        collider,
                        rigid_body,
                        &mut physics_resource.bodies,
                    ));
                }
                None => {
                    log::info!("Creating a static collider");
                    physics.collider = Some(
                        physics_resource
                            .colliders
                            .insert(collider.position(transform.0)),
                    );
                }
            }
        }
    }
}

pub fn step_physics(mut physics: ResMut<PhysicsResource>) {
    let PhysicsResource {
        bodies,
        colliders,
        ref integration_parameters,
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
        integration_parameters,
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

pub fn update_rigid_body_transforms(
    mut query: Query<(&mut TransformComponent, &PhysicsComponent)>,
    game_physics: Res<PhysicsResource>,
) {
    for (mut transform, physics) in query.iter_mut() {
        if let Some(handle) = physics.rigid_body {
            let body = &game_physics.bodies[handle];
            transform.0 = Isometry::from_parts((*body.translation()).into(), *body.rotation());
        }
    }
}
