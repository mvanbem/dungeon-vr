use std::convert::Infallible;

use dungeon_vr_stream_codec::{ExternalStreamCodec, StreamCodec, UnframedByteVec};

use crate::packet::ReadPacketError;

pub struct VoicePacket {
    pub data: Vec<u8>,
}

impl StreamCodec for VoicePacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let data = UnframedByteVec::read_from_ext(r)?;
        Ok(Self { data })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        UnframedByteVec::write_to_ext(w, &self.data)?;
        Ok(())
    }
}
