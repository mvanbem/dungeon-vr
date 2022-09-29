use bevy_ecs::prelude::*;
use rapier3d::prelude::*;

use crate::collider_cache::{BorrowedColliderCacheKey, ColliderCache};
use crate::core::{Authority, LocalAuthorityResource, TransformComponent};

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

    pub fn destroy(self, game_physics: &mut PhysicsResource) {
        if let Some(collider) = self.collider {
            game_physics.colliders.remove(
                collider,
                &mut game_physics.islands,
                &mut game_physics.bodies,
                false,
            );
        }
        if let Some(rigid_body) = self.rigid_body {
            game_physics.bodies.remove(
                rigid_body,
                &mut game_physics.islands,
                &mut game_physics.colliders,
                &mut game_physics.impulse_joints,
                &mut game_physics.multibody_joints,
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

pub fn sync_physics(
    mut query: Query<(&Authority, &TransformComponent, &mut PhysicsComponent)>,
    local_authority: Res<LocalAuthorityResource>,
    mut physics_resource: ResMut<PhysicsResource>,
) {
    // Dereference so that struct fields can be borrowed independently.
    let physics_resource = &mut *physics_resource;

    for (authority, transform, mut physics) in query.iter_mut() {
        // Determine the rigid body goal state.
        struct RigidBodyParams {
            ccd_enabled: bool,
        }
        let goal = match physics.mode {
            NetPhysicsMode::Dynamic { ccd_enabled } if *authority == local_authority.0 => {
                // Dynamic rigid body.
                Some(RigidBodyParams { ccd_enabled })
            }
            NetPhysicsMode::Dynamic { .. } => {
                // Kinematic rigid body.
                Some(RigidBodyParams { ccd_enabled: false })
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
                rigid_body.enable_ccd(goal.ccd_enabled)
            }
            (Some(handle), None) => {
                // Rigid body is present and unwanted. Remove it.
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
            }
            (None, Some(goal)) => {
                // Rigid body is not present, but is wanted. Create it.
                physics.rigid_body = Some(
                    physics_resource.bodies.insert(
                        RigidBodyBuilder::dynamic()
                            .translation(transform.0.translation.vector)
                            .ccd_enabled(goal.ccd_enabled)
                            .build(),
                    ),
                );
                // If there is already a collider, associate it with the new rigid body.
                if let Some(collider) = physics.collider {
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
            let collider = physics_resource
                .collider_cache
                .get(BorrowedColliderCacheKey::ConvexHull(&physics.collider_name));
            physics.collider = Some(match physics.rigid_body {
                Some(rigid_body) => physics_resource.colliders.insert_with_parent(
                    collider,
                    rigid_body,
                    &mut physics_resource.bodies,
                ),
                None => physics_resource.colliders.insert(collider),
            });
        }
    }
}
