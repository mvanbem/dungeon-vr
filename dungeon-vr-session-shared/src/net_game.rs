use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::fmt::{self, Display, Formatter};
use std::num::{NonZeroU32, NonZeroU8};

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ReadError, ReadStringError, StreamCodec};
use rapier3d::na::Isometry3;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ReadSnapshotError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("{0}")]
    ReadStringError(#[from] ReadStringError),

    #[error("invalid zero net ID")]
    InvalidNetId,

    #[error("invalid game state token: 0x{0:02x}")]
    InvalidGameStateToken(u8),
}

/// A small nonzero integer identifying a player currently connected to a game. A player's ID does
/// not change while they are connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(pub NonZeroU8);

impl Display for PlayerId {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Player {}", self.0.get())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Component)]
pub struct NetId(pub NonZeroU32);

impl StreamCodec for NetId {
    type ReadError = ReadSnapshotError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadSnapshotError> {
        match NonZeroU32::new(u32::read_from(r)?) {
            None => Err(ReadSnapshotError::InvalidNetId),
            Some(id) => Ok(Self(id)),
        }
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.0.get().write_to(w)
    }
}

// #[derive(Component)]
// struct Player {
//     player_id: PlayerId,
// }

#[derive(Clone, Copy, PartialEq, Eq, Component)]
pub enum Authority {
    Server,
    Client(PlayerId),
}

impl StreamCodec for Authority {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        Ok(match NonZeroU8::new(u8::read_from(r)?) {
            None => Self::Server,
            Some(id) => Self::Client(PlayerId(id)),
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        match self {
            Self::Server => 0,
            Self::Client(id) => id.0.get(),
        }
        .write_to(w)
    }
}

#[derive(Bundle)]
pub struct Replicated {
    pub net_id: NetId,
    pub authority: Authority,
}

#[derive(Component, Default)]
pub struct Transform(pub Isometry3<f32>);

#[derive(Component)]
pub struct ModelName(pub String);

pub enum Input {
    /// Claim authority over an unclaimed object.
    Claim { net_id: NetId },
    /// Yield authority of a claimed object.
    Yield { net_id: NetId },
}

pub fn apply_inputs(
    mut authority_query: Query<&mut Authority>,
    entities_by_net_id: Res<BTreeMap<NetId, Entity>>,
    player_inputs: Res<BTreeMap<PlayerId, Vec<Input>>>,
) {
    for (&player_id, inputs) in &*player_inputs {
        for input in inputs {
            match input {
                Input::Claim { net_id } => {
                    let mut authority = authority_query
                        .get_mut(entities_by_net_id[&net_id])
                        .unwrap();
                    if *authority == Authority::Server {
                        *authority = Authority::Client(player_id);
                    } else {
                        // The claim is either a no-op or is invalid. No action required.
                    }
                }
                Input::Yield { net_id } => {
                    let mut authority = authority_query
                        .get_mut(entities_by_net_id[&net_id])
                        .unwrap();
                    if *authority == Authority::Client(player_id) {
                        *authority = Authority::Server;
                    } else {
                        // The yield is invalid. No action required.
                    }
                }
            }
        }
    }
}

pub fn write_snapshot(w: &mut Vec<u8>, world: &mut World) -> Result<(), Infallible> {
    // Gather a sorted list entities by net ID.
    let mut entities_by_net_id = world
        .query::<(&NetId, Entity)>()
        .iter(world)
        .map(|(&net_id, entity)| (net_id, entity))
        .collect::<Vec<_>>();
    entities_by_net_id.sort_unstable_by_key(|&(net_id, _)| net_id);

    (entities_by_net_id.len() as u32).write_to(w)?;
    for (net_id, entity) in entities_by_net_id {
        let entity = world.entity(entity);
        net_id.write_to(w)?;
        entity.get::<Authority>().unwrap().write_to(w)?;
        if let Some(transform) = entity.get::<Transform>() {
            1u8.write_to(w)?;
            transform.0.write_to(w)?;
        }
        if let Some(model_name) = entity.get::<ModelName>() {
            2u8.write_to(w)?;
            model_name.0.write_to(w)?;
        }
        0u8.write_to(w)?;
    }
    Ok(())
}

pub fn apply_snapshot(r: &mut &[u8], world: &mut World) -> Result<(), ReadSnapshotError> {
    let count = u32::read_from(r)?;
    let entities_by_net_id = world
        .query::<(&NetId, Entity)>()
        .iter(world)
        .map(|(&net_id, entity)| (net_id, entity))
        .collect::<HashMap<_, _>>();

    for _ in 0..count {
        // Get or create the referenced entity.
        let net_id = NetId::read_from(r)?;
        let mut entity = match entities_by_net_id.get(&net_id).copied() {
            Some(entity) => world.entity_mut(entity),
            None => {
                let entity = world.spawn().id();
                world
                    .resource_mut::<BTreeMap<NetId, Entity>>()
                    .insert(net_id, entity);

                let mut entity = world.entity_mut(entity);
                entity.insert(net_id);
                entity
            }
        };

        // Update the authority component.
        let authority = Authority::read_from(r)?;
        entity.insert(authority);

        // Update any other components defined in the snapshot.
        loop {
            match u8::read_from(r)? {
                0 => break,
                1 => {
                    let transform = Isometry3::<f32>::read_from(r)?;
                    entity.insert(Transform(transform));
                }
                2 => {
                    let name = String::read_from(r)?;
                    entity.insert(ModelName(name));
                }
                token => return Err(ReadSnapshotError::InvalidGameStateToken(token)),
            }
        }
    }
    Ok(())
}
