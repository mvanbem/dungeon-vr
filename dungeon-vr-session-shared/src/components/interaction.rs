use bevy_ecs::prelude::*;

use crate::components::net::NetId;

#[derive(Component)]
pub struct Hand {
    pub index: usize,
    pub grab_state: HandGrabState,
}

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

#[derive(Component)]
pub struct Grabbable {
    pub grabbed: bool,
}
