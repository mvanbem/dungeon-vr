use std::convert::Infallible;

use dungeon_vr_stream_codec::StreamCodec;

use crate::packet::ReadPacketError;
use crate::time::{ClientTime, ServerTime};

pub struct PongPacket {
    pub client_time: ClientTime,
    pub server_time: ServerTime,
}

impl StreamCodec for PongPacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let client_time = ClientTime::from_nanos_since_epoch(u64::read_from(r)?);
        let server_time = ServerTime::from_nanos_since_epoch(u64::read_from(r)?);
        Ok(Self {
            client_time,
            server_time,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.client_time.to_nanos_since_epoch().write_to(w)?;
        self.server_time.to_nanos_since_epoch().write_to(w)?;
        Ok(())
    }
}
