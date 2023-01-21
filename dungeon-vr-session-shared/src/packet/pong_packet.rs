use std::convert::Infallible;

use dungeon_vr_stream_codec::StreamCodec;

use crate::packet::ReadPacketError;
use crate::time::{ClientTime, NanoDuration, ServerTime};
use crate::TickId;

pub struct PongPacket {
    pub client_time: ClientTime,
    pub server_time: ServerTime,
    pub server_last_completed_tick: TickId,
    pub server_tick_interval: NanoDuration,
}

impl StreamCodec for PongPacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let client_time = ClientTime::from_nanos_since_epoch(i64::read_from(r)?);
        let server_time = ServerTime::from_nanos_since_epoch(i64::read_from(r)?);
        let server_last_completed_tick = TickId(u32::read_from(r)?);
        let server_tick_interval = NanoDuration::from_nanos(i64::read_from(r)?);
        Ok(Self {
            client_time,
            server_time,
            server_last_completed_tick,
            server_tick_interval,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.client_time.as_nanos_since_epoch().write_to(w)?;
        self.server_time.as_nanos_since_epoch().write_to(w)?;
        self.server_last_completed_tick.0.write_to(w)?;
        self.server_tick_interval.as_nanos().write_to(w)?;
        Ok(())
    }
}
