use std::convert::Infallible;

use byteorder::{ReadBytesExt, WriteBytesExt};
use rapier3d::na::{self as nalgebra, Isometry3, UnitQuaternion};
use rapier3d::na::{vector, Quaternion, Vector3};

use crate::{eof, ReadError, StreamCodec, O};

impl StreamCodec for Vector3<f32> {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        let x = eof(r.read_f32::<O>())?;
        let y = eof(r.read_f32::<O>())?;
        let z = eof(r.read_f32::<O>())?;
        Ok(vector![x, y, z])
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        w.write_f32::<O>(self.x).unwrap();
        w.write_f32::<O>(self.y).unwrap();
        w.write_f32::<O>(self.z).unwrap();
        Ok(())
    }
}

impl StreamCodec for Quaternion<f32> {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        let w = eof(r.read_f32::<O>())?;
        let i = eof(r.read_f32::<O>())?;
        let j = eof(r.read_f32::<O>())?;
        let k = eof(r.read_f32::<O>())?;
        Ok(Quaternion::new(w, i, j, k))
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        w.write_f32::<O>(self.w).unwrap();
        w.write_f32::<O>(self.i).unwrap();
        w.write_f32::<O>(self.j).unwrap();
        w.write_f32::<O>(self.k).unwrap();
        Ok(())
    }
}

impl StreamCodec for Isometry3<f32> {
    type ReadError = ReadError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadError> {
        let translation = Vector3::read_from(r)?.into();
        let rotation = UnitQuaternion::new_unchecked(Quaternion::read_from(r)?);
        Ok(Isometry3::from_parts(translation, rotation))
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        self.translation.vector.write_to(w)?;
        self.rotation.quaternion().write_to(w)?;
        Ok(())
    }
}
