use std::convert::Infallible;
use std::io::{Read, Write};

use byteorder::{ReadBytesExt, WriteBytesExt};
use paste::paste;
use thiserror::Error;

use crate::{eof, ExternalStreamCodec, ReadError, StreamCodec, O};

impl StreamCodec for () {
    type ReadError = Infallible;
    type WriteError = Infallible;

    fn read_from(_r: &mut &[u8]) -> Result<Self, Infallible> {
        Ok(())
    }

    fn write_to(&self, _w: &mut Vec<u8>) -> Result<(), Infallible> {
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum ReadBoolError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("invalid bool encoding 0x{0:02x}")]
    InvalidEncoding(u8),
}

impl StreamCodec for bool {
    type ReadError = ReadBoolError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadBoolError> {
        match u8::read_from(r)? {
            0 => Ok(false),
            1 => Ok(true),
            x => Err(ReadBoolError::InvalidEncoding(x)),
        }
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        w.write_u8(if *self { 1 } else { 0 }).unwrap();
        Ok(())
    }
}

impl StreamCodec for u8 {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        eof(r.read_u8())
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        Ok(w.write_u8(*self).unwrap())
    }
}

impl StreamCodec for i8 {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        eof(r.read_i8())
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        Ok(w.write_i8(*self).unwrap())
    }
}

impl<const N: usize> StreamCodec for [u8; N] {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        let mut value = [0; N];
        eof(r.read_exact(&mut value))?;
        Ok(value)
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        Ok(w.write_all(self).unwrap())
    }
}

macro_rules! impl_stream_codec_for_int {
    ($t:ty) => {
        paste! {
            impl StreamCodec for $t {
                type ReadError = ReadError;
                type WriteError = Infallible;

                fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
                    eof(r.[<read_ $t>]::<O>())
                }

                fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
                    Ok(w.[<write_ $t>]::<O>(*self).unwrap())
                }
            }
        }
    };
}

impl_stream_codec_for_int!(u16);
impl_stream_codec_for_int!(u32);
impl_stream_codec_for_int!(u64);
impl_stream_codec_for_int!(i16);
impl_stream_codec_for_int!(i32);
impl_stream_codec_for_int!(i64);

pub enum UnframedByteVec {}

impl ExternalStreamCodec for UnframedByteVec {
    type Item = Vec<u8>;
    type ReadError = Infallible;
    type WriteError = Infallible;

    fn read_from_ext(r: &mut &[u8]) -> Result<Vec<u8>, Infallible> {
        let mut value = Vec::new();
        r.read_to_end(&mut value).unwrap();
        Ok(value)
    }

    fn write_to_ext(w: &mut Vec<u8>, value: &Vec<u8>) -> Result<(), Infallible> {
        Ok(w.write_all(value).unwrap())
    }
}
