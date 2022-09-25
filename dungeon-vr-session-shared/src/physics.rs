use rapier3d::prelude::*;

use crate::collider_cache::ColliderCache;

pub struct GamePhysics {
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
