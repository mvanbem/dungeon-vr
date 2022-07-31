use std::convert::Infallible;

use dungeon_vr_stream_codec::StreamCodec;
use rand_core::{OsRng, RngCore};

use crate::protocol::packet::ReadPacketError;

/// A challenge token, a block of random data the client must echo to demonstrate its ability to
/// receive and send packets at its address and its knowledge of the shared secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChallengeToken {
    data: [u8; Self::SIZE],
}

impl ChallengeToken {
    pub const SIZE: usize = 256;

    pub fn gen() -> Self {
        let mut data = [0; Self::SIZE];
        OsRng.fill_bytes(&mut data[..]);
        Self { data }
    }

    pub fn data(&self) -> &[u8; Self::SIZE] {
        &self.data
    }
}

impl StreamCodec for ChallengeToken {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        Ok(Self {
            data: <[u8; Self::SIZE]>::read_from(r)?,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.data.write_to(w)
    }
}
