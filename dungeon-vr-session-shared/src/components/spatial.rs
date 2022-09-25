use bevy_ecs::prelude::*;
use rapier3d::na::Isometry3;

#[derive(Component, Default)]
pub struct Transform(pub Isometry3<f32>);

#[derive(Component)]
pub struct FliesAround;
