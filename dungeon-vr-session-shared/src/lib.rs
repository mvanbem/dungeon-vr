#![feature(mixed_integer_ops)]

use std::convert::Infallible;
use std::fmt::{self, Display, Formatter};
use std::num::NonZeroU8;
use std::time::Duration;

use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use thiserror::Error;

use crate::time::ServerTime;

pub mod action;
pub mod collider_cache;
pub mod components;
pub mod packet;
pub mod physics;
pub mod resources;
pub mod snapshot;
pub mod systems;
pub mod time;

pub const TICK_RATE: Duration = Duration::from_millis(50); // 20 Hz

/// A small nonzero integer identifying a player currently connected to a game. A player's ID does
/// not change while they are connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(pub NonZeroU8);

impl Display for PlayerId {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Player {}", self.0.get())
    }
}

#[derive(Error, Debug)]
pub enum ReadPlayerIdError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("invalid zero player ID")]
    InvalidPlayerId,
}

impl StreamCodec for PlayerId {
    type ReadError = ReadPlayerIdError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPlayerIdError> {
        let id = u8::read_from(r)?;
        let id = NonZeroU8::new(id).ok_or_else(|| ReadPlayerIdError::InvalidPlayerId)?;
        Ok(Self(id))
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.0.get().write_to(w)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TickId(pub u32);

impl TickId {
    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).unwrap())
    }

    pub fn latest_tick_for_time(time: ServerTime) -> Self {
        const TICK_RATE_MICROS: u64 = TICK_RATE.as_micros() as u64;
        Self((time.to_micros_since_epoch() / TICK_RATE_MICROS) as u32)
    }

    pub fn goal_time(self) -> ServerTime {
        const TICK_RATE_MICROS: u64 = TICK_RATE.as_micros() as u64;
        ServerTime::from_micros_since_epoch(self.0 as u64 * TICK_RATE_MICROS)
    }
}
