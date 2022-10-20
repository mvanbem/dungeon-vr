use std::collections::BTreeMap;
use std::convert::Infallible;

use dungeon_vr_stream_codec::StreamCodec;

use crate::action::Action;
use crate::packet::ReadPacketError;
use crate::TickId;

pub struct CommitActionsPacket {
    pub actions_by_tick_id: BTreeMap<TickId, Vec<Action>>,
}

impl StreamCodec for CommitActionsPacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let count = u8::read_from(r)?;
        let mut actions_by_tick_id = BTreeMap::new();
        for _ in 0..count {
            let tick_id = TickId(u32::read_from(r)?);
            let mut actions = Vec::new();
            let count = u8::read_from(r)?;
            for _ in 0..count {
                actions.push(Action::read_from(r)?);
            }
            actions_by_tick_id.insert(tick_id, actions);
        }
        Ok(Self { actions_by_tick_id })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        u8::try_from(self.actions_by_tick_id.len())
            .unwrap()
            .write_to(w)?;
        for (tick_id, actions) in &self.actions_by_tick_id {
            tick_id.0.write_to(w)?;
            u8::try_from(actions.len()).unwrap().write_to(w)?;
            for action in actions {
                action.write_to(w)?;
            }
        }
        Ok(())
    }
}
