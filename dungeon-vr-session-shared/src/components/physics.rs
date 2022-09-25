use bevy_ecs::prelude::*;
use rapier3d::prelude::{ColliderHandle, RigidBodyHandle};

#[derive(Clone, Copy, Debug, Component)]
pub struct Physics {
    pub collider: ColliderHandle,
    pub rigid_body: Option<RigidBodyHandle>,
}
