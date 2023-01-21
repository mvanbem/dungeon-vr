use std::convert::Infallible;

use dungeon_vr_stream_codec::{ExternalStreamCodec, StreamCodec, UnframedByteVec};

use crate::packet::ReadPacketError;
use crate::time::NanoDuration;
use crate::TickId;

pub struct GameStatePacket {
    pub tick_id: TickId,
    pub tick_interval: NanoDuration,
    pub serialized_game_state: Vec<u8>,
}

impl StreamCodec for GameStatePacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let tick_id = TickId(u32::read_from(r)?);
        let tick_interval = NanoDuration::from_nanos(i64::read_from(r)?);
        let serialized_game_state = UnframedByteVec::read_from_ext(r)?;
        Ok(Self {
            tick_id,
            tick_interval,
            serialized_game_state,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.tick_id.0.write_to(w)?;
        self.tick_interval.as_nanos().write_to(w)?;
        UnframedByteVec::write_to_ext(w, &self.serialized_game_state)?;
        Ok(())
    }
}
