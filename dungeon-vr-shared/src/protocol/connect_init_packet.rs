use std::convert::Infallible;

use dungeon_vr_cryptography::PublicKey;
use dungeon_vr_stream_codec::StreamCodec;

use crate::protocol::packet::ReadPacketError;

/// The initial packet from a client that wants to connect.
pub struct ConnectInitPacket {
    /// The Game ID, which must be [`GAME_ID`](crate::protocol::GAME_ID) to be accepted.
    pub game_id: u64,
    /// The client's public key for ECDH key exchange.
    pub client_public_key: PublicKey,
}

impl StreamCodec for ConnectInitPacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let game_id = u64::read_from(r)?;
        let client_public_key = PublicKey::read_from(r)?;
        Ok(Self {
            game_id,
            client_public_key,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.game_id.write_to(w)?;
        self.client_public_key.write_to(w)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use dungeon_vr_cryptography::PrivateKey;
    use dungeon_vr_stream_codec::StreamCodec;

    use crate::protocol::packet::Packet;

    use super::ConnectInitPacket;

    #[test]
    fn round_trip() {
        let client_public_key = PrivateKey::gen().to_public();

        let mut w = Vec::new();
        Packet::ConnectInit(ConnectInitPacket {
            game_id: 0x0123456789abcdef,
            client_public_key,
        })
        .write_to(&mut w)
        .unwrap();

        let mut r = &w[..];
        let packet = Packet::read_from(&mut r).unwrap();
        assert!(r.is_empty());
        let packet = match packet {
            Packet::ConnectInit(packet) => packet,
            _ => unreachable!(),
        };
        assert_eq!(packet.game_id, 0x0123456789abcdef);
        assert_eq!(packet.client_public_key, client_public_key);
    }
}
