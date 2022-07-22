use openxr as xr;
use rapier3d::na::{Isometry3, Quaternion, UnitQuaternion, Vector3};

pub fn xr_posef_to_na_isometry(pose: xr::Posef) -> Isometry3<f32> {
    Isometry3::from_parts(
        Vector3::new(pose.position.x, pose.position.y, pose.position.z).into(),
        UnitQuaternion::new_unchecked(Quaternion::new(
            pose.orientation.w,
            pose.orientation.x,
            pose.orientation.y,
            pose.orientation.z,
        )),
    )
}
