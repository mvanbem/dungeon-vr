use std::convert::Infallible;
use std::fmt::{self, Debug, Formatter};
use std::io::Write;

use chacha20poly1305::aead::rand_core::{OsRng, RngCore};
use chacha20poly1305::aead::{Aead, NewAead};
use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum KeyExchangeError {
    #[error("non-contributory key exchange")]
    NonContributory,
}

#[derive(Error, Debug)]
#[error("error in authenticated decryption")]
pub struct DecryptError;

#[derive(Clone)]
pub struct PrivateKey(x25519_dalek::ReusableSecret);

impl Debug for PrivateKey {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "PrivateKey(_)")
    }
}

impl PrivateKey {
    pub fn gen() -> Self {
        Self(x25519_dalek::ReusableSecret::new(rand_core::OsRng))
    }

    pub fn to_public(&self) -> PublicKey {
        PublicKey(x25519_dalek::PublicKey::from(&self.0))
    }

    pub fn exchange(&self, public_key: &PublicKey) -> Result<SharedSecret, KeyExchangeError> {
        let shared_secret = self.0.diffie_hellman(&public_key.0);
        if !shared_secret.was_contributory() {
            return Err(KeyExchangeError::NonContributory);
        }
        Ok(SharedSecret(shared_secret.to_bytes().into()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey(x25519_dalek::PublicKey);

impl Debug for PublicKey {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "PublicKey(_)")
    }
}

impl StreamCodec for PublicKey {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        Ok(Self(<[u8; 32]>::read_from(r)?.into()))
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.0.to_bytes().write_to(w)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SharedSecret(chacha20poly1305::Key);

impl Debug for SharedSecret {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "SharedSecret(_)")
    }
}

impl SharedSecret {
    pub fn gen() -> Self {
        Self(chacha20poly1305::XChaCha20Poly1305::generate_key(OsRng))
    }

    pub fn encrypt(&self, plaintext: &[u8], nonce: &Nonce) -> Vec<u8> {
        chacha20poly1305::XChaCha20Poly1305::new(&self.0)
            .encrypt(&nonce.0, plaintext)
            .unwrap()
    }

    pub fn decrypt(&self, ciphertext: &[u8], nonce: &Nonce) -> Result<Vec<u8>, DecryptError> {
        chacha20poly1305::XChaCha20Poly1305::new(&self.0)
            .decrypt(&nonce.0, ciphertext)
            .map_err(|_| DecryptError)
    }
}

#[derive(Clone, Copy)]
pub struct Nonce(chacha20poly1305::XNonce);

impl Nonce {
    pub fn gen() -> Self {
        let mut buf = [0; 24];
        OsRng.fill_bytes(&mut buf);
        Self(buf.into())
    }
}

impl Debug for Nonce {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Nonce(_)")
    }
}

impl StreamCodec for Nonce {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        Ok(Self(<[u8; 24]>::read_from(r)?.into()))
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        Ok(w.write_all(&self.0).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use chacha20poly1305::aead::rand_core::{OsRng, RngCore};

    use super::{Nonce, PrivateKey, SharedSecret};

    #[test]
    fn key_exchange() {
        let private_key_a = PrivateKey::gen();
        let public_key_a = private_key_a.to_public();
        let private_key_b = PrivateKey::gen();
        let public_key_b = private_key_b.to_public();

        let shared_key_a = private_key_a.exchange(&public_key_b).unwrap();
        let shared_key_b = private_key_b.exchange(&public_key_a).unwrap();

        assert_eq!(shared_key_a.0, shared_key_b.0);
    }

    #[test]
    fn authenticated_encryption() {
        let plaintext = {
            let mut data = [0; 300];
            OsRng.fill_bytes(&mut data);
            data
        };
        let nonce = Nonce::gen();
        let key = SharedSecret::gen();

        let ciphertext = key.encrypt(&plaintext, &nonce);
        let round_trip_plaintext = key.decrypt(&ciphertext, &nonce).unwrap();

        assert_eq!(&plaintext[..], &round_trip_plaintext);
    }
}
