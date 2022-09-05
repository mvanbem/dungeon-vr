use std::mem::MaybeUninit;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use rapier3d::na::{self as nalgebra, UnitVector3, Vector3};
use rapier3d::prelude::vector;
use steam_audio_sys::*;

#[derive(Clone)]
pub struct SteamAudioContext {
    inner: Arc<InnerContext>,
}

struct InnerContext {
    context: IPLContext,
    audio_settings: IPLAudioSettings,
    hrtf: IPLHRTF,
    out_buffer: Mutex<IPLAudioBuffer>,
}

unsafe impl Send for InnerContext {}
unsafe impl Sync for InnerContext {}

impl SteamAudioContext {
    pub fn new() -> Result<SteamAudioContext> {
        unsafe {
            let mut context = null_mut();
            match iplContextCreate(
                &mut IPLContextSettings {
                    version: STEAMAUDIO_VERSION,
                    logCallback: None,
                    allocateCallback: None,
                    freeCallback: None,
                    simdLevel: IPLSIMDLevel_IPL_SIMDLEVEL_AVX512,
                },
                &mut context,
            ) {
                x if x == IPLerror_IPL_STATUS_SUCCESS => (),
                x => bail!("iplContextCreate() returned {x}"),
            }

            let mut hrtf_settings = IPLHRTFSettings {
                type_: IPLHRTFType_IPL_HRTFTYPE_DEFAULT,
                sofaFileName: null(),
            };

            let mut audio_settings = IPLAudioSettings {
                samplingRate: 48000,
                frameSize: 1024,
            };

            let mut hrtf = null_mut();
            match iplHRTFCreate(context, &mut audio_settings, &mut hrtf_settings, &mut hrtf) {
                x if x == IPLerror_IPL_STATUS_SUCCESS => (),
                x => bail!("iplHRTFCreate() returned {x}"),
            }

            let mut out_buffer = MaybeUninit::uninit();
            match iplAudioBufferAllocate(
                context,
                2,
                audio_settings.frameSize,
                out_buffer.as_mut_ptr(),
            ) {
                x if x == IPLerror_IPL_STATUS_SUCCESS => (),
                x => bail!("iplAudioBufferAllocate() returned {x}"),
            }
            let out_buffer = Mutex::new(out_buffer.assume_init());

            Ok(Self {
                inner: Arc::new(InnerContext {
                    context,
                    audio_settings,
                    hrtf,
                    out_buffer,
                }),
            })
        }
    }

    pub fn binaural_effect(&self) -> BinauralEffect {
        BinauralEffect::new(Arc::clone(&self.inner)).unwrap()
    }

    pub fn calculate_relative_direction(
        &self,
        source_position: Vector3<f32>,
        listener_position: Vector3<f32>,
        listener_ahead: UnitVector3<f32>,
        listener_up: UnitVector3<f32>,
    ) -> UnitVector3<f32> {
        unsafe {
            let result = iplCalculateRelativeDirection(
                self.inner.context,
                IPLVector3 {
                    x: source_position.x,
                    y: source_position.y,
                    z: source_position.z,
                },
                IPLVector3 {
                    x: listener_position.x,
                    y: listener_position.y,
                    z: listener_position.z,
                },
                IPLVector3 {
                    x: listener_ahead.x,
                    y: listener_ahead.y,
                    z: listener_ahead.z,
                },
                IPLVector3 {
                    x: listener_up.x,
                    y: listener_up.y,
                    z: listener_up.z,
                },
            );

            UnitVector3::new_unchecked(vector![result.x, result.y, result.z])
        }
    }
}

impl Drop for InnerContext {
    fn drop(&mut self) {
        unsafe {
            iplAudioBufferFree(self.context, &mut *self.out_buffer.get_mut().unwrap());
            iplHRTFRelease(&mut self.hrtf);
            iplContextRelease(&mut self.context);
        }
    }
}

pub struct BinauralEffect {
    context: Arc<InnerContext>,
    effect: IPLBinauralEffect,
}

unsafe impl Send for BinauralEffect {}
unsafe impl Sync for BinauralEffect {}

impl BinauralEffect {
    fn new(context: Arc<InnerContext>) -> Result<Self> {
        unsafe {
            let mut effect_settings = IPLBinauralEffectSettings { hrtf: context.hrtf };

            let mut effect = null_mut();
            match iplBinauralEffectCreate(
                context.context,
                // Not actually mutated.
                &context.audio_settings as *const IPLAudioSettings as *mut IPLAudioSettings,
                &mut effect_settings,
                &mut effect,
            ) {
                x if x == IPLerror_IPL_STATUS_SUCCESS => (),
                x => bail!("iplBinauralEffectCreate() returned {x}"),
            }

            Ok(Self { context, effect })
        }
    }

    pub fn apply(
        &mut self,
        mono_samples: &[f32],
        direction: UnitVector3<f32>,
        interleaved_stereo_samples: &mut [f32],
    ) {
        assert_eq!(
            mono_samples.len(),
            self.context.audio_settings.frameSize as usize,
        );
        assert_eq!(
            interleaved_stereo_samples.len(),
            2 * self.context.audio_settings.frameSize as usize,
        );

        unsafe {
            let mut effect_params = IPLBinauralEffectParams {
                direction: IPLVector3 {
                    x: direction.x,
                    y: direction.y,
                    z: direction.z,
                },
                interpolation: IPLHRTFInterpolation_IPL_HRTFINTERPOLATION_BILINEAR,
                spatialBlend: 1.0,
                hrtf: self.context.hrtf,
                peakDelays: null_mut(),
            };
            let mut in_data = mono_samples.as_ptr() as *mut f32;
            let mut in_buffer = IPLAudioBuffer {
                numChannels: 1,
                numSamples: self.context.audio_settings.frameSize,
                data: &mut in_data,
            };
            let mut out_buffer = self.context.out_buffer.lock().unwrap();
            _ = iplBinauralEffectApply(
                self.effect,
                &mut effect_params,
                &mut in_buffer,
                &mut *out_buffer,
            );

            iplAudioBufferInterleave(
                self.context.context,
                &mut *out_buffer,
                interleaved_stereo_samples.as_mut_ptr(),
            );
        }
    }
}

impl Drop for BinauralEffect {
    fn drop(&mut self) {
        unsafe {
            iplBinauralEffectRelease(&mut self.effect);
        }
    }
}
