use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::fmt::{self, Display, Formatter};
use std::num::{NonZeroU32, NonZeroU8};

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ExternalStreamCodec, ReadError, StreamCodec};
use rapier3d::na::{self as nalgebra, Isometry3, Unit, UnitQuaternion, Vector3};
use rapier3d::prelude::vector;
use thiserror::Error;

use crate::packet::TickId;

#[derive(Error, Debug)]
pub enum ReadNetGameError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NetId(NonZeroU32);

impl StreamCodec for NetId {
    type ReadError = ReadNetGameError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadNetGameError> {
        match NonZeroU32::new(u32::read_from(r)?) {
            None => Err(ReadNetGameError::InvalidNetId),
            Some(id) => Ok(Self(id)),
        }
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.0.get().write_to(w)
    }
}

#[derive(Component)]
struct Replicated {
    net_id: NetId,
    authority: Authority,
}

#[derive(Component)]
struct Player {
    player_id: PlayerId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Authority {
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

#[derive(Component, Default)]
struct Transform(Isometry3<f32>);

#[derive(Component)]
struct FliesAround;

pub enum Input {
    /// Claim authority over an unclaimed object.
    Claim { net_id: NetId },
    /// Yield authority of a claimed object.
    Yield { net_id: NetId },
}

pub struct NetGame {
    world: World,
    schedule: Schedule,
    entities_by_net_id: BTreeMap<NetId, Entity>,
    tick_id: TickId,
}

impl NetGame {
    pub fn new() -> Self {
        let mut world = World::new();
        let mut entities_by_net_id = BTreeMap::new();

        let net_id = NetId(NonZeroU32::new(1).unwrap());
        let entity = world
            .spawn()
            .insert(Replicated {
                net_id,
                authority: Authority::Server,
            })
            .insert(Transform::default())
            .insert(FliesAround)
            .id();
        entities_by_net_id.insert(net_id, entity);

        Self {
            world,
            schedule: Schedule::default().with_stage(
                "update",
                SystemStage::parallel()
                    .with_system(apply_player_inputs)
                    .with_system(fly_around),
            ),
            entities_by_net_id,
            tick_id: TickId(0),
        }
    }

    pub fn update(&mut self, player_inputs: BTreeMap<PlayerId, Vec<Input>>) {
        self.tick_id = TickId(self.tick_id.0 + 1);

        self.world.insert_resource(self.entities_by_net_id.clone());
        self.world.insert_resource(player_inputs);
        self.world.insert_resource(self.tick_id);
        self.schedule.run(&mut self.world);
        self.world.remove_resource::<HashMap<NetId, Entity>>();
        self.world
            .remove_resource::<HashMap<PlayerId, Vec<Input>>>();
    }

    pub fn where_is_the_object(&mut self) -> Isometry3<f32> {
        for transform in self.world.query::<&Transform>().iter(&self.world) {
            eprintln!("got transform: {:?}", transform.0);
            return transform.0;
        }
        unreachable!();
    }
}

fn apply_player_inputs(
    mut replicated_query: Query<&mut Replicated>,
    entities_by_net_id: Res<BTreeMap<NetId, Entity>>,
    player_inputs: Res<BTreeMap<PlayerId, Vec<Input>>>,
) {
    for (&player_id, inputs) in &*player_inputs {
        for input in inputs {
            match input {
                Input::Claim { net_id } => {
                    let mut replicated = replicated_query
                        .get_mut(entities_by_net_id[&net_id])
                        .unwrap();
                    if replicated.authority == Authority::Server {
                        replicated.authority = Authority::Client(player_id);
                    } else {
                        // The claim is either a no-op or is invalid. No action required.
                    }
                }
                Input::Yield { net_id } => {
                    let mut replicated = replicated_query
                        .get_mut(entities_by_net_id[&net_id])
                        .unwrap();
                    if replicated.authority == Authority::Client(player_id) {
                        replicated.authority = Authority::Server;
                    } else {
                        // The yield is invalid. No action required.
                    }
                }
            }
        }
    }
}

fn fly_around(mut query: Query<&mut Transform, With<FliesAround>>, tick_id: Res<TickId>) {
    let t = tick_id.0 as f32 * 0.05; // arbitrary. TODO: Relate to real time.
    for mut transform in query.iter_mut() {
        transform.0.translation.vector = Vector3::zeros();
        transform.0.rotation =
            UnitQuaternion::from_axis_angle(&Unit::new_unchecked(vector![0.0, 1.0, 0.0]), t);
    }
}

pub enum NetGameFullCodec {}

impl ExternalStreamCodec for NetGameFullCodec {
    type Item = NetGame;
    type ReadError = ReadNetGameError;
    type WriteError = Infallible;

    fn read_from_ext(r: &mut &[u8]) -> Result<NetGame, ReadNetGameError> {
        let count = u32::read_from(r)?;
        let mut game = NetGame::new();
        game.world.clear_entities();
        game.entities_by_net_id.clear();
        for _ in 0..count {
            let mut entity = game.world.spawn();
            let net_id = NetId::read_from(r)?;
            let authority = Authority::read_from(r)?;
            entity.insert(Replicated { net_id, authority });
            loop {
                match u8::read_from(r)? {
                    0 => break,
                    1 => {
                        let transform = Isometry3::<f32>::read_from(r)?;
                        entity.insert(Transform(transform));
                    }
                    token => return Err(ReadNetGameError::InvalidGameStateToken(token)),
                }
            }
        }
        Ok(game)
    }

    fn write_to_ext(w: &mut Vec<u8>, value: &NetGame) -> Result<(), Infallible> {
        // Gather a sorted list of all net IDs.
        let mut net_ids = value
            .entities_by_net_id
            .keys()
            .copied()
            .collect::<Vec<NetId>>();
        net_ids.sort_by_key(|x| x.0);

        (net_ids.len() as u32).write_to(w)?;
        for net_id in net_ids {
            let entity = value.world.entity(value.entities_by_net_id[&net_id]);
            let replicated = entity.get::<Replicated>().unwrap();
            net_id.write_to(w)?;
            replicated.authority.write_to(w)?;
            if let Some(transform) = entity.get::<Transform>() {
                1u8.write_to(w)?;
                transform.0.write_to(w)?;
            }
            0u8.write_to(w)?;
        }
        Ok(())
    }
}
