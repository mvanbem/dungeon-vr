use std::collections::HashMap;
use std::convert::Infallible;

use dungeon_vr_stream_codec::StreamCodec;
use rapier3d::prelude::*;

use crate::core::NetId;
use crate::packet::ReadPacketError;
use crate::TickId;

pub struct UpdateOwnedTransformsPacket {
    pub after_tick_id: TickId,
    pub transforms_by_net_id: HashMap<NetId, Isometry<f32>>,
}

impl StreamCodec for UpdateOwnedTransformsPacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let after_tick_id = TickId(u32::read_from(r)?);
        let count = u32::read_from(r)?;
        let mut transforms_by_net_id = HashMap::new();
        for _ in 0..count {
            let net_id = NetId::read_from(r)?;
            let transform = Isometry::read_from(r)?;
            transforms_by_net_id.insert(net_id, transform);
        }
        Ok(Self {
            after_tick_id,
            transforms_by_net_id,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.after_tick_id.0.write_to(w)?;
        u32::try_from(self.transforms_by_net_id.len())
            .unwrap()
            .write_to(w)?;
        for (net_id, transform) in &self.transforms_by_net_id {
            net_id.write_to(w)?;
            transform.write_to(w)?;
        }
        Ok(())
    }
}
