use std::io::{self};

use thiserror::Error;

mod nalgebra_impls;
mod std_impls;

type O = byteorder::BigEndian;

pub use crate::std_impls::{ReadBoolError, ReadStringError, UnframedByteVec};

#[derive(Error, Debug)]
pub enum ReadError {
    #[error("unexpected end of input")]
    UnexpectedEof,
}

impl From<ReadError> for io::Error {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::UnexpectedEof => Self::new(io::ErrorKind::UnexpectedEof, e),
        }
    }
}

fn eof<T>(e: Result<T, io::Error>) -> Result<T, ReadError> {
    e.map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => ReadError::UnexpectedEof,
        _ => unreachable!(),
    })
}

pub trait StreamCodec: Sized {
    type ReadError;
    type WriteError;

    fn read_from(r: &mut &[u8]) -> Result<Self, Self::ReadError>;
    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Self::WriteError>;
}

pub trait ExternalStreamCodec {
    type Item;
    type ReadError;
    type WriteError;

    fn read_from_ext(r: &mut &[u8]) -> Result<Self::Item, Self::ReadError>;
    fn write_to_ext(w: &mut Vec<u8>, value: &Self::Item) -> Result<(), Self::WriteError>;
}

impl<C> ExternalStreamCodec for C
where
    C: StreamCodec,
{
    type Item = C;
    type ReadError = C::ReadError;
    type WriteError = C::WriteError;

    fn read_from_ext(r: &mut &[u8]) -> Result<C, Self::ReadError> {
        C::read_from(r)
    }

    fn write_to_ext(w: &mut Vec<u8>, value: &C) -> Result<(), Self::WriteError> {
        value.write_to(w)
    }
}
