use std::convert::Infallible;

use dungeon_vr_stream_codec::StreamCodec;

use crate::packet::ReadPacketError;
use crate::time::ClientTime;

pub struct PingPacket {
    pub client_time: ClientTime,
}

impl StreamCodec for PingPacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let client_time = ClientTime::from_micros_since_epoch(u64::read_from(r)?);
        Ok(Self { client_time })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.client_time.to_micros_since_epoch().write_to(w)?;
        Ok(())
    }
}
