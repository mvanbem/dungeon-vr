use std::convert::Infallible;

use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use thiserror::Error;

use crate::action::ReadActionError;
use crate::core::ReadNetIdError;
use crate::packet::commit_actions_packet::CommitActionsPacket;
use crate::packet::game_state_packet::GameStatePacket;
use crate::packet::ping_packet::PingPacket;
use crate::packet::player_assignment_packet::PlayerAssignmentPacket;
use crate::packet::pong_packet::PongPacket;
use crate::packet::update_owned_transforms_packet::UpdateOwnedTransformsPacket;
use crate::packet::voice_packet::VoicePacket;
use crate::ReadPlayerIdError;

pub mod commit_actions_packet;
pub mod game_state_packet;
pub mod ping_packet;
pub mod player_assignment_packet;
pub mod pong_packet;
pub mod update_owned_transforms_packet;
pub mod voice_packet;

#[derive(Debug, Error)]
pub enum ReadPacketError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("{0}")]
    ReadPlayerIdError(#[from] ReadPlayerIdError),

    #[error("{0}")]
    ReadActionError(#[from] ReadActionError),

    #[error("{0}")]
    ReadNetIdError(#[from] ReadNetIdError),

    #[error("invalid packet type encoding: 0x{0:02x}")]
    InvalidPacketType(u8),

    #[error("unexpected trailing data")]
    TrailingData,
}

impl From<Infallible> for ReadPacketError {
    fn from(e: Infallible) -> Self {
        match e {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketKind {
    Ping,
    Pong,
    GameState,
    Voice,
    PlayerAssignment,
    CommitActions,
    UpdateOwnedTransforms,
}

impl StreamCodec for PacketKind {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        u8::read_from(r)?.try_into()
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        (*self as u8).write_to(w)
    }
}

impl TryFrom<u8> for PacketKind {
    type Error = ReadPacketError;

    fn try_from(value: u8) -> Result<Self, ReadPacketError> {
        match value {
            x if x == Self::Ping as u8 => Ok(Self::Ping),
            x if x == Self::Pong as u8 => Ok(Self::Pong),
            x if x == Self::GameState as u8 => Ok(Self::GameState),
            x if x == Self::Voice as u8 => Ok(Self::Voice),
            x if x == Self::PlayerAssignment as u8 => Ok(Self::PlayerAssignment),
            x if x == Self::CommitActions as u8 => Ok(Self::CommitActions),
            x if x == Self::UpdateOwnedTransforms as u8 => Ok(Self::UpdateOwnedTransforms),
            x => Err(ReadPacketError::InvalidPacketType(x)),
        }
    }
}

pub enum Packet {
    Ping(PingPacket),
    Pong(PongPacket),
    GameState(GameStatePacket),
    Voice(VoicePacket),
    PlayerAssignment(PlayerAssignmentPacket),
    CommitActions(CommitActionsPacket),
    UpdateOwnedTransforms(UpdateOwnedTransformsPacket),
}

impl Packet {
    pub fn kind(&self) -> PacketKind {
        match self {
            Self::Ping(_) => PacketKind::Ping,
            Self::Pong(_) => PacketKind::Pong,
            Self::GameState(_) => PacketKind::GameState,
            Self::Voice(_) => PacketKind::Voice,
            Self::PlayerAssignment(_) => PacketKind::PlayerAssignment,
            Self::CommitActions(_) => PacketKind::CommitActions,
            Self::UpdateOwnedTransforms(_) => PacketKind::UpdateOwnedTransforms,
        }
    }
}

impl StreamCodec for Packet {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        match PacketKind::read_from(r)? {
            PacketKind::Ping => Ok(Self::Ping(PingPacket::read_from(r)?)),
            PacketKind::Pong => Ok(Self::Pong(PongPacket::read_from(r)?)),
            PacketKind::GameState => Ok(Self::GameState(GameStatePacket::read_from(r)?)),
            PacketKind::Voice => Ok(Self::Voice(VoicePacket::read_from(r)?)),
            PacketKind::PlayerAssignment => Ok(Self::PlayerAssignment(
                PlayerAssignmentPacket::read_from(r)?,
            )),
            PacketKind::CommitActions => {
                Ok(Self::CommitActions(CommitActionsPacket::read_from(r)?))
            }
            PacketKind::UpdateOwnedTransforms => Ok(Self::UpdateOwnedTransforms(
                UpdateOwnedTransformsPacket::read_from(r)?,
            )),
        }
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.kind().write_to(w)?;
        match self {
            Self::Ping(packet) => packet.write_to(w),
            Self::Pong(packet) => packet.write_to(w),
            Self::GameState(packet) => packet.write_to(w),
            Self::Voice(packet) => packet.write_to(w),
            Self::PlayerAssignment(packet) => packet.write_to(w),
            Self::CommitActions(packet) => packet.write_to(w),
            Self::UpdateOwnedTransforms(packet) => packet.write_to(w),
        }
    }
}
