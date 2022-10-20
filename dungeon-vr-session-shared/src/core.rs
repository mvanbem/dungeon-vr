use std::convert::Infallible;
use std::num::{NonZeroU32, NonZeroU8};

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use rapier3d::prelude::*;
use thiserror::Error;

use crate::{NetComponent, PlayerId};

#[derive(Error, Debug)]
pub enum ReadNetIdError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("invalid zero net ID")]
    InvalidNetId,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub enum Authority {
    Server,
    Player(PlayerId),
}

impl StreamCodec for Authority {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        Ok(match NonZeroU8::new(u8::read_from(r)?) {
            None => Self::Server,
            Some(id) => Self::Player(PlayerId(id)),
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        match self {
            Self::Server => 0,
            Self::Player(id) => id.0.get(),
        }
        .write_to(w)
    }
}

/// Component tracking a synchronized entity's [`NetId`] and [`Authority`].
#[derive(Clone, Debug, Component)]
pub struct SynchronizedComponent {
    pub net_id: NetId,
    pub authority: Authority,
}

pub struct LocalAuthorityResource(pub Option<Authority>);

impl LocalAuthorityResource {
    pub fn is_local(&self, synchronized: Option<&SynchronizedComponent>) -> bool {
        match (self.0, synchronized) {
            // Synchronized entities in an online session are local only if their authority matches
            // the local authority.
            (Some(local_authority), Some(synchronized)) => {
                synchronized.authority == local_authority
            }
            // Everything else is local.
            _ => true,
        }
    }
}

#[derive(Debug, Default, Component)]
pub struct TransformComponent(pub Isometry<f32>);

impl NetComponent for TransformComponent {}
