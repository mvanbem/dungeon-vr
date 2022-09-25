use std::convert::Infallible;

use dungeon_vr_stream_codec::StreamCodec;

use crate::packet::ReadPacketError;
use crate::PlayerId;

pub struct PlayerAssignmentPacket {
    pub player_id: PlayerId,
}

impl StreamCodec for PlayerAssignmentPacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let player_id = PlayerId::read_from(r)?;
        Ok(Self { player_id })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.player_id.write_to(w)?;
        Ok(())
    }
}
