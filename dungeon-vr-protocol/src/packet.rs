use std::convert::Infallible;

use dungeon_vr_cryptography::DecryptError;
use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use thiserror::Error;

use crate::challenge_token::ChallengeToken;
use crate::connect_challenge_packet::ConnectChallengePacket;
use crate::connect_init_packet::ConnectInitPacket;
use crate::sealed::Sealed;

#[derive(Debug, Error)]
pub enum ReadPacketError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("{0}")]
    DecryptError(#[from] DecryptError),

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
    Disconnect,
    ConnectInit,
    ConnectChallenge,
    ConnectResponse,
    Keepalive,
    GameData,
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
            x if x == Self::Disconnect as u8 => Ok(Self::Disconnect),
            x if x == Self::ConnectInit as u8 => Ok(Self::ConnectInit),
            x if x == Self::ConnectChallenge as u8 => Ok(Self::ConnectChallenge),
            x if x == Self::ConnectResponse as u8 => Ok(Self::ConnectResponse),
            x if x == Self::Keepalive as u8 => Ok(Self::Keepalive),
            x if x == Self::GameData as u8 => Ok(Self::GameData),
            x => Err(ReadPacketError::InvalidPacketType(x)),
        }
    }
}

pub enum Packet {
    Disconnect(Sealed<()>),
    ConnectInit(ConnectInitPacket),
    ConnectChallenge(ConnectChallengePacket),
    ConnectResponse(Sealed<ChallengeToken>),
    Keepalive(Sealed<()>),
    GameData(Sealed<Vec<u8>>),
}

impl Packet {
    pub fn kind(&self) -> PacketKind {
        match self {
            Self::Disconnect(_) => PacketKind::Disconnect,
            Self::ConnectInit(_) => PacketKind::ConnectInit,
            Self::ConnectChallenge(_) => PacketKind::ConnectChallenge,
            Self::ConnectResponse(_) => PacketKind::ConnectResponse,
            Self::Keepalive(_) => PacketKind::Keepalive,
            Self::GameData(_) => PacketKind::GameData,
        }
    }
}

impl StreamCodec for Packet {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        match PacketKind::read_from(r)? {
            PacketKind::Disconnect => Ok(Self::Disconnect(Sealed::read_from(r)?)),
            PacketKind::ConnectInit => Ok(Self::ConnectInit(ConnectInitPacket::read_from(r)?)),
            PacketKind::ConnectChallenge => Ok(Self::ConnectChallenge(
                ConnectChallengePacket::read_from(r)?,
            )),
            PacketKind::ConnectResponse => Ok(Self::ConnectResponse(Sealed::read_from(r)?)),
            PacketKind::Keepalive => Ok(Self::Keepalive(Sealed::read_from(r)?)),
            PacketKind::GameData => Ok(Self::GameData(Sealed::read_from(r)?)),
        }
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.kind().write_to(w)?;
        match self {
            Self::Disconnect(packet) => packet.write_to(w),
            Self::ConnectInit(packet) => packet.write_to(w),
            Self::ConnectChallenge(packet) => packet.write_to(w),
            Self::ConnectResponse(packet) => packet.write_to(w),
            Self::Keepalive(packet) => packet.write_to(w),
            Self::GameData(packet) => packet.write_to(w),
        }
    }
}

#[cfg(test)]
mod tests {
    use dungeon_vr_cryptography::SharedSecret;
    use dungeon_vr_stream_codec::StreamCodec;

    use crate::challenge_token::ChallengeToken;
    use crate::sealed::Sealed;

    use super::Packet;

    #[test]
    fn round_trip() {
        let shared_secret = SharedSecret::gen();
        let token = ChallengeToken::gen();

        let mut w = Vec::new();
        Packet::ConnectResponse(Sealed::seal(token, &shared_secret))
            .write_to(&mut w)
            .unwrap();

        let mut r = &w[..];
        let packet = Packet::read_from(&mut r).unwrap();
        assert!(r.is_empty());
        let roundtrip_token = match packet {
            Packet::ConnectResponse(sealed) => sealed.open(&shared_secret).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(roundtrip_token, token);
    }
}
