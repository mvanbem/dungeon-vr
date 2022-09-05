#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

include!(concat!(env!("OUT_DIR"), "/bindgen.rs"));

pub const STEAMAUDIO_VERSION: u32 = (STEAMAUDIO_VERSION_MAJOR as u32) << 16
    | (STEAMAUDIO_VERSION_MINOR as u32) << 8
    | STEAMAUDIO_VERSION_PATCH as u32;
