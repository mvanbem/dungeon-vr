use std::convert::Infallible;

use dungeon_vr_cryptography::PublicKey;
use dungeon_vr_stream_codec::StreamCodec;

use crate::challenge_token::ChallengeToken;
use crate::packet::ReadPacketError;
use crate::sealed::Sealed;

/// The server's response to a valid
/// [`ConnectInitPacket`](crate::connect_init_packet::ConnectInitPacket).
pub struct ConnectChallengePacket {
    /// The server's public key for ECDH key exchange.
    pub server_public_key: PublicKey,
    /// The encrypted part of the packet.
    pub sealed_payload: Sealed<ChallengeToken>,
}

impl StreamCodec for ConnectChallengePacket {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let server_public_key = PublicKey::read_from(r)?;
        let sealed_payload = Sealed::read_from(r)?;
        Ok(Self {
            server_public_key,
            sealed_payload,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.server_public_key.write_to(w)?;
        self.sealed_payload.write_to(w)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use dungeon_vr_cryptography::PrivateKey;
    use dungeon_vr_stream_codec::StreamCodec;

    use crate::challenge_token::ChallengeToken;
    use crate::packet::Packet;
    use crate::sealed::Sealed;

    use super::ConnectChallengePacket;

    #[test]
    fn round_trip() {
        let client_private_key = PrivateKey::gen();
        let server_private_key = PrivateKey::gen();
        let client_public_key = client_private_key.to_public();
        let server_public_key = server_private_key.to_public();
        let token = ChallengeToken::gen();

        let mut w = Vec::new();
        Packet::ConnectChallenge(ConnectChallengePacket {
            server_public_key,
            sealed_payload: Sealed::seal(
                token,
                &server_private_key.exchange(&client_public_key).unwrap(),
            ),
        })
        .write_to(&mut w)
        .unwrap();

        let mut r = &w[..];
        let packet = Packet::read_from(&mut r).unwrap();
        assert!(r.is_empty());
        let packet = match packet {
            Packet::ConnectChallenge(packet) => packet,
            _ => unreachable!(),
        };
        assert_eq!(packet.server_public_key, server_public_key);
        let roundtrip_token = packet
            .sealed_payload
            .open(
                &client_private_key
                    .exchange(&packet.server_public_key)
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(roundtrip_token, token);
    }
}
