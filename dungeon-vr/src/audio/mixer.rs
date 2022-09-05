use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use bevy_ecs::prelude::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleRate, Stream, StreamConfig};
use rapier3d::na::{self as nalgebra, vector, Isometry3, UnitVector3, Vector3};

use crate::audio::steam_audio::{self, SteamAudioContext};

pub struct Mixer {
    inner: Arc<Mutex<InnerMixer>>,
    _output_stream: Stream,
}

struct InnerMixer {
    steam_audio: SteamAudioContext,
    world: World,
    listener_transform: Isometry3<f32>,
    last_print_instant: std::time::Instant,
}

pub struct Sound {
    samples: Box<[f32]>,
}

impl Sound {
    pub fn from_samples(samples: Box<[f32]>) -> Self {
        assert_eq!(samples.len() % 1024, 0);
        Self { samples }
    }
}

#[derive(Component)]
struct Source(Arc<Sound>);

#[derive(Component)]
struct SampleOffset(usize);

#[derive(Component)]
struct Position(Vector3<f32>);

#[derive(Component)]
struct Looped(bool);

#[derive(Component)]
struct Gain(f32);

#[derive(Component)]
struct BinauralEffect(steam_audio::BinauralEffect);

#[derive(Bundle)]
struct SourceBundle {
    source: Source,
    sample_offset: SampleOffset,
    looped: Looped,
    position: Position,
    gain: Gain,
    binaural_effect: BinauralEffect,
}

pub struct SourceKey(Entity);

impl Mixer {
    pub fn new() -> Result<Self> {
        let steam_audio = SteamAudioContext::new().context("initializing Steam Audio")?;

        let inner = Arc::new(Mutex::new(InnerMixer {
            steam_audio,
            world: World::new(),
            listener_transform: Isometry3::default(),
            last_print_instant: std::time::Instant::now(),
        }));

        let host = cpal::default_host();

        let input_device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no audio input device"))?;
        log::info!("Audio input device: {}", input_device.name()?);
        for cfg in input_device.supported_input_configs()? {
            log::info!("Supported audio input configuration: {:?}", cfg);
        }

        let output_device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no audio output device"))?;
        log::info!("Audio output device: {}", output_device.name()?);
        for cfg in output_device.supported_output_configs()? {
            log::info!("Supported audio output configuration: {:?}", cfg);
        }

        let output_config = StreamConfig {
            channels: 2,
            sample_rate: SampleRate(48000),
            buffer_size: BufferSize::Default,
        };
        let output_stream = output_device
            .build_output_stream(
                &output_config,
                Self::audio_output_callback(Arc::clone(&inner))?,
                move |err| {
                    log::error!("Audio output error: {err}");
                },
            )
            .context("building audio output stream")?;
        output_stream
            .play()
            .context("playing audio output stream")?;

        Ok(Self {
            inner,
            _output_stream: output_stream,
        })
    }

    fn audio_output_callback(
        inner: Arc<Mutex<InnerMixer>>,
    ) -> Result<impl FnMut(&mut [f32], &cpal::OutputCallbackInfo)> {
        let mut frame = [0.0; 2048];
        let mut offset = 0;
        Ok(
            move |mut data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                while !data.is_empty() {
                    // Mix another audio frame if the current one is empty.
                    if offset >= frame.len() {
                        inner.lock().unwrap().mix_frame(&mut frame);
                        offset = 0;
                    }

                    // Copy and consume as much audio data as will fit.
                    let len = data.len().min(frame.len() - offset);
                    assert!(len > 0);
                    data[..len].copy_from_slice(&frame[offset..offset + len]);
                    data = &mut data[len..];
                    offset += len;
                }
            },
        )
    }

    pub fn play(
        &self,
        sound: Arc<Sound>,
        sample_offset: usize,
        looped: bool,
        position: Vector3<f32>,
        gain: f32,
    ) -> SourceKey {
        let mut inner = self.inner.lock().unwrap();
        let binaural_effect = inner.steam_audio.binaural_effect();
        let mut entity = inner.world.spawn();
        entity.insert_bundle(SourceBundle {
            source: Source(sound),
            sample_offset: SampleOffset(sample_offset),
            looped: Looped(looped),
            position: Position(position),
            gain: Gain(gain),
            binaural_effect: BinauralEffect(binaural_effect),
        });
        SourceKey(entity.id())
    }

    pub fn set_listener_transform(&self, transform: Isometry3<f32>) {
        self.inner.lock().unwrap().listener_transform = transform;
    }
}

impl InnerMixer {
    fn mix_frame(&mut self, frame: &mut [f32]) {
        assert_eq!(frame.len(), 2 * 1024);

        frame.fill(0.0);

        let listener_position = self.listener_transform.translation.vector;
        let listener_ahead = UnitVector3::new_unchecked(
            self.listener_transform
                .transform_vector(&vector![0.0, 0.0, -1.0]),
        );
        let listener_up = UnitVector3::new_unchecked(
            self.listener_transform
                .transform_vector(&vector![0.0, 1.0, 0.0]),
        );

        let mut buf = [0.0; 2048];
        let mut entities_to_remove = Vec::new();
        for (entity, source, mut sample_offset, looped, position, gain, mut binaural_effect) in self
            .world
            .query::<(
                Entity,
                &Source,
                &mut SampleOffset,
                &Looped,
                &Position,
                &Gain,
                &mut BinauralEffect,
            )>()
            .iter_mut(&mut self.world)
        {
            // Apply the binaural effect to the current frame of the source.
            let direction = self.steam_audio.calculate_relative_direction(
                position.0,
                listener_position,
                listener_ahead,
                listener_up,
            );
            let now = std::time::Instant::now();
            if now.duration_since(self.last_print_instant) >= std::time::Duration::from_millis(100)
            {
                self.last_print_instant = now;
                println!("dir: {direction:?}");
            }
            binaural_effect.0.apply(
                &source.0.samples[sample_offset.0..sample_offset.0 + 1024],
                direction,
                &mut buf,
            );

            // Mix the result.
            for i in 0..2048 {
                frame[i] = gain.0 * buf[i];
            }

            // Advance the source to the next frame.
            sample_offset.0 += 1024;

            // Wrap or remove the source if past the end.
            if sample_offset.0 >= source.0.samples.len() {
                if looped.0 {
                    sample_offset.0 = 0;
                } else {
                    entities_to_remove.push(entity);
                }
            }
        }

        // Clean up any one-shot sources that have stopped playing.
        for entity in entities_to_remove {
            self.world.despawn(entity);
        }
    }
}
