use bevy_ecs::prelude::*;

use crate::core::NetId;
use crate::NetComponent;

#[derive(Debug, Component)]
pub struct HandComponent {
    pub index: usize,
    pub grab_state: HandGrabState,
}

impl NetComponent for HandComponent {}

#[derive(Clone, Copy, Debug)]
pub enum HandGrabState {
    Empty,
    Grabbing(NetId),
}

impl HandGrabState {
    pub fn grab_target(self) -> Option<NetId> {
        match self {
            Self::Empty => None,
            Self::Grabbing(net_id) => Some(net_id),
        }
    }
}

#[derive(Debug, Component)]
pub struct GrabbableComponent {
    pub grabbed: bool,
}

impl NetComponent for GrabbableComponent {}
