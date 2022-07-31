use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::num::{NonZeroU32, NonZeroU8};

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ExternalStreamCodec, ReadError, StreamCodec};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ReadNetGameError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("invalid zero net ID")]
    InvalidNetId,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientId(pub NonZeroU8);

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
    client_id: ClientId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Authority {
    Server,
    Client(ClientId),
}

impl StreamCodec for Authority {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        Ok(match NonZeroU8::new(u8::read_from(r)?) {
            None => Self::Server,
            Some(id) => Self::Client(ClientId(id)),
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
}

impl NetGame {
    pub fn new() -> Self {
        Self {
            world: World::new(),
            schedule: Schedule::default().with_stage(
                "update",
                SystemStage::parallel().with_system(apply_player_inputs),
            ),
            entities_by_net_id: BTreeMap::new(),
        }
    }

    pub fn update(&mut self, player_inputs: BTreeMap<ClientId, Vec<Input>>) {
        self.world.insert_resource(self.entities_by_net_id.clone());
        self.world.insert_resource(player_inputs);
        self.schedule.run(&mut self.world);
        self.world.remove_resource::<HashMap<NetId, Entity>>();
        self.world
            .remove_resource::<HashMap<ClientId, Vec<Input>>>();
    }
}

fn apply_player_inputs(
    mut replicated_query: Query<&mut Replicated>,
    entities_by_net_id: Res<BTreeMap<NetId, Entity>>,
    player_inputs: Res<BTreeMap<ClientId, Vec<Input>>>,
) {
    for (&client_id, inputs) in &*player_inputs {
        for input in inputs {
            match input {
                Input::Claim { net_id } => {
                    let mut replicated = replicated_query
                        .get_mut(entities_by_net_id[&net_id])
                        .unwrap();
                    if replicated.authority == Authority::Server {
                        replicated.authority = Authority::Client(client_id);
                    } else {
                        // The claim is either a no-op or is invalid. No action required.
                    }
                }
                Input::Yield { net_id } => {
                    let mut replicated = replicated_query
                        .get_mut(entities_by_net_id[&net_id])
                        .unwrap();
                    if replicated.authority == Authority::Client(client_id) {
                        replicated.authority = Authority::Server;
                    } else {
                        // The yield is invalid. No action required.
                    }
                }
            }
        }
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
        for _ in 0..count {
            let net_id = NetId::read_from(r)?;
            let authority = Authority::read_from(r)?;
            game.world.spawn().insert(Replicated { net_id, authority });
            // TODO: Decode any other components.
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
            // TODO: Encode any other components relevant to replication.
        }
        Ok(())
    }
}
