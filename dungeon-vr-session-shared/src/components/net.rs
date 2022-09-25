use std::convert::Infallible;
use std::num::{NonZeroU32, NonZeroU8};

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use thiserror::Error;

use crate::PlayerId;

#[derive(Error, Debug)]
pub enum ReadNetIdError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("invalid zero net ID")]
    InvalidNetId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Component)]
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

#[derive(Bundle)]
pub struct Replicated {
    pub net_id: NetId,
    pub authority: Authority,
}
