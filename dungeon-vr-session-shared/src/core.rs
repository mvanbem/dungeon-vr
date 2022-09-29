use std::convert::Infallible;
use std::num::{NonZeroU32, NonZeroU8};

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use rapier3d::na::Isometry3;
use thiserror::Error;

use crate::PlayerId;

#[derive(Error, Debug)]
pub enum ReadNetIdError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("invalid zero net ID")]
    InvalidNetId,
}

/// Component tracking a synchronized entity's [`NetId`].
#[derive(Clone, Copy, Debug, Component)]
pub struct Synchronized {
    pub net_id: NetId,
}

/// A unique integer assigned by the server to each synchronized entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NetId(pub NonZeroU32);

impl StreamCodec for NetId {
    type ReadError = ReadNetIdError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadNetIdError> {
        match NonZeroU32::new(u32::read_from(r)?) {
            None => Err(ReadNetIdError::InvalidNetId),
            Some(id) => Ok(Self(id)),
        }
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.0.get().write_to(w)
    }
}

/// Component tracking authority for entities that can be owned by a player.
#[derive(Clone, Copy, Debug, Component)]
pub struct AuthorityComponent(Authority);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
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

pub struct LocalAuthorityResource(pub Authority);

#[derive(Component, Default)]
pub struct TransformComponent(pub Isometry3<f32>);
