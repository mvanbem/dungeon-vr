use bevy_ecs::prelude::*;
use rapier3d::na::{self as nalgebra, vector, Unit, UnitQuaternion};

use crate::core::TransformComponent;
use crate::NetComponent;

#[derive(Component)]
pub struct FlyAroundComponent;

impl NetComponent for FlyAroundComponent {}

pub fn fly_around(mut query: Query<&mut TransformComponent, With<FlyAroundComponent>>) {
    const DT: f32 = 1.0 / 20.0;
    for mut transform in query.iter_mut() {
        transform.0.translation.vector = vector![0.0, 1.0, 0.0];
        transform.0.rotation *=
            UnitQuaternion::from_axis_angle(&Unit::new_unchecked(vector![0.0, 1.0, 0.0]), DT);
    }
}
