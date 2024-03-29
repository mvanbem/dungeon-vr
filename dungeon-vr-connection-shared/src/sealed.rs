use std::convert::Infallible;
use std::io::{Read, Write};
use std::marker::PhantomData;

use dungeon_vr_cryptography::{Nonce, SharedSecret};
use dungeon_vr_stream_codec::{ExternalStreamCodec, StreamCodec};

use crate::packet::ReadPacketError;

pub struct Sealed<P> {
    nonce: Nonce,
    data: Vec<u8>,
    _phantom_t: PhantomData<P>,
}

impl<P> Sealed<P> {
    pub fn cast<Q>(self) -> Sealed<Q> {
        Sealed {
            nonce: self.nonce,
            data: self.data,
            _phantom_t: PhantomData,
        }
    }
}

impl<P> Sealed<P>
where
    P: StreamCodec<WriteError = Infallible>,
    <P as StreamCodec>::ReadError: Into<ReadPacketError>,
{
    pub fn seal(packet: P, shared_secret: &SharedSecret) -> Self {
        let mut plaintext = Vec::new();
        packet.write_to(&mut plaintext).unwrap();

        let nonce = Nonce::gen();
        let data = shared_secret.encrypt(&plaintext, &nonce);
        Self {
            nonce,
            data,
            _phantom_t: PhantomData,
        }
    }

    pub fn open(&self, shared_secret: &SharedSecret) -> Result<P, ReadPacketError> {
        let plaintext = shared_secret.decrypt(&self.data[..], &self.nonce)?;
        let mut r = &*plaintext;
        let packet = P::read_from(&mut r).map_err(Into::into)?;
        if !r.is_empty() {
            return Err(ReadPacketError::TrailingData.into());
        }
        Ok(packet)
    }
}

impl<P> Sealed<P> {
    pub fn seal_ext<C>(packet: P, shared_secret: &SharedSecret) -> Self
    where
        C: ExternalStreamCodec<Item = P, WriteError = Infallible>,
    {
        let mut plaintext = Vec::new();
        C::write_to_ext(&mut plaintext, &packet).unwrap();

        let nonce = Nonce::gen();
        let data = shared_secret.encrypt(&plaintext, &nonce);
        Self {
            nonce,
            data,
            _phantom_t: PhantomData,
        }
    }

    pub fn open_ext<C>(&self, shared_secret: &SharedSecret) -> Result<P, ReadPacketError>
    where
        C: ExternalStreamCodec<Item = P, WriteError = Infallible>,
        <C as ExternalStreamCodec>::ReadError: Into<ReadPacketError>,
    {
        let plaintext = shared_secret.decrypt(&self.data[..], &self.nonce)?;
        let mut r = &*plaintext;
        let packet = C::read_from_ext(&mut r).map_err(Into::into)?;
        if !r.is_empty() {
            return Err(ReadPacketError::TrailingData.into());
        }
        Ok(packet)
    }
}

impl<P> StreamCodec for Sealed<P> {
    type ReadError = ReadPacketError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadPacketError> {
        let nonce = Nonce::read_from(r)?;
        let mut data = Vec::new();
        r.read_to_end(&mut data).unwrap();
        Ok(Self {
            nonce,
            data,
            _phantom_t: PhantomData,
        })
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.nonce.write_to(w)?;
        w.write_all(&self.data).unwrap();
        Ok(())
    }
}
