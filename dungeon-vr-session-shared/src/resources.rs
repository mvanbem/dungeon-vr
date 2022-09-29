use std::collections::{BTreeMap, HashMap};

use bevy_ecs::prelude::*;

use crate::action::Action;
use crate::core::NetId;
use crate::PlayerId;

#[derive(Default)]
pub struct AllActions(pub HashMap<PlayerId, Vec<Action>>);

#[derive(Default)]
pub struct EntitiesByNetId(pub BTreeMap<NetId, Entity>);
