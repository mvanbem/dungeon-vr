use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::Range;
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, SampleRate, StreamConfig};
use dungeon_vr_connection_client::ConnectionClient;
use dungeon_vr_session_client::{Event as SessionEvent, Request as SessionRequest, SessionClient};
use tokio::net::UdpSocket;
use tokio::select;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Connects to a remote server at this ip:port.
    connect: String,
}

#[tokio::main]
pub async fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_target(false)
        .format_timestamp_micros()
        .init();
    let args = Args::parse();

    let server_addr = SocketAddr::from_str(&args.connect).unwrap();
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.connect(server_addr).await?;
    let (_connection_cancel_guard, connection_requests, connection_events) =
        ConnectionClient::spawn(Box::new(socket));
    let mut session_client = SessionClient::new(connection_requests, connection_events);

    let mut audio_ctx = AudioContext::new()?;
    let mut voice_from_microphone = audio_ctx.take_voice_from_microphone().unwrap();
    let voice_to_speakers = audio_ctx.take_voice_to_speakers().unwrap();

    let cancel_token = set_ctrlc_handler();

    while !cancel_token.is_cancelled() {
        select! {
            biased;

            _ = cancel_token.cancelled() => break,

            data = voice_from_microphone.recv() => if let Some(data) = data {
                let _ = session_client.try_send_request(SessionRequest::SendVoice(data));
            },

            event = session_client.recv_event() => match event {
                SessionEvent::Voice(data) => {
                    let _ = voice_to_speakers.try_send(data);
                }
                _ => (),
            },
        }
    }
    Ok(())
}

struct AudioContext {
    _input_stream: cpal::Stream,
    _output_stream: cpal::Stream,
    voice_from_microphone: Option<mpsc::Receiver<Vec<u8>>>,
    voice_to_speakers: Option<mpsc::Sender<Vec<u8>>>,
}

impl AudioContext {
    fn new() -> Result<Self> {
        let host = cpal::default_host();

        let (voice_from_microphone_tx, voice_from_microphone_rx) = mpsc::channel(100);
        let (voice_to_speakers_tx, voice_to_speakers_rx) = mpsc::channel(100);

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

        let mut input_mono = false;
        let mut input_stereo = false;
        for cfg in input_device.supported_input_configs()? {
            if cfg.channels() == 1
                && (cfg.min_sample_rate()..=cfg.max_sample_rate()).contains(&SampleRate(48000))
                && cfg.sample_format() == SampleFormat::F32
            {
                input_mono = true;
            }
            if cfg.channels() == 2
                && (cfg.min_sample_rate()..=cfg.max_sample_rate()).contains(&SampleRate(48000))
                && cfg.sample_format() == SampleFormat::F32
            {
                input_stereo = true;
            }
        }
        let input_config = if input_mono {
            StreamConfig {
                channels: 1,
                sample_rate: SampleRate(48000),
                buffer_size: BufferSize::Default,
            }
        } else if input_stereo {
            StreamConfig {
                channels: 2,
                sample_rate: SampleRate(48000),
                buffer_size: BufferSize::Default,
            }
        } else {
            bail!("Unable to find a preferred format for audio input.");
        };
        let input_stream = input_device
            .build_input_stream(
                &input_config,
                Self::audio_input_callback(&input_config, voice_from_microphone_tx)?,
                move |err| {
                    log::error!("Audio input error: {err}");
                },
            )
            .context("building audio input stream")?;
        input_stream.play().context("playing audio input stream")?;

        let output_config = StreamConfig {
            channels: 2,
            sample_rate: SampleRate(48000),
            buffer_size: BufferSize::Default,
        };
        let output_stream = output_device
            .build_output_stream(
                &output_config,
                Self::audio_output_callback(voice_to_speakers_rx)?,
                move |err| {
                    log::error!("Audio output error: {err}");
                },
            )
            .context("building audio output stream")?;
        output_stream
            .play()
            .context("playing audio output stream")?;

        Ok(Self {
            _input_stream: input_stream,
            _output_stream: output_stream,
            voice_from_microphone: Some(voice_from_microphone_rx),
            voice_to_speakers: Some(voice_to_speakers_tx),
        })
    }

    fn take_voice_from_microphone(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.voice_from_microphone.take()
    }

    fn take_voice_to_speakers(&mut self) -> Option<mpsc::Sender<Vec<u8>>> {
        self.voice_to_speakers.take()
    }

    fn audio_input_callback(
        stream_config: &StreamConfig,
        packets: mpsc::Sender<Vec<u8>>,
    ) -> Result<impl FnMut(&[f32], &cpal::InputCallbackInfo)> {
        let channels = stream_config.channels as usize;
        const SAMPLES_PER_FRAME: usize = 960;
        const MAX_PACKET_SIZE: usize = 1024;
        let mut buf = Vec::with_capacity(SAMPLES_PER_FRAME);
        let mut encoder = opus::Encoder::new(48000, opus::Channels::Mono, opus::Application::Voip)?;
        Ok(move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            for frame in data.chunks(channels) {
                // Downmix multi-channel sources.
                let mut combined_sample = 0.0;
                for sample in frame.iter().copied() {
                    combined_sample += sample;
                }
                combined_sample /= channels as f32;

                buf.push(combined_sample);
                if buf.len() == SAMPLES_PER_FRAME {
                    // Ready to encode an Opus frame.
                    let packet = encoder
                        .encode_vec_float(buf.as_slice(), MAX_PACKET_SIZE)
                        .unwrap();
                    buf.clear();
                    _ = packets.try_send(packet);
                }
            }
        })
    }

    fn audio_output_callback(
        mut packets: mpsc::Receiver<Vec<u8>>,
    ) -> Result<impl FnMut(&mut [f32], &cpal::OutputCallbackInfo)> {
        const BUF_SIZE: usize = 960;
        let mut decoder = opus::Decoder::new(48000, opus::Channels::Mono)?;
        let mut buf = [0.0; BUF_SIZE];
        let mut buf_range = 0..0;

        fn fill_buf(
            pipe_rx: &mut mpsc::Receiver<Vec<u8>>,
            decoder: &mut opus::Decoder,
            buf: &mut [f32; BUF_SIZE],
            buf_range: &mut Range<usize>,
        ) -> Result<(), ()> {
            assert!(Range::<usize>::is_empty(buf_range));
            match pipe_rx.try_recv() {
                Ok(packet) => {
                    decoder
                        .decode_float(&packet, buf.as_mut_slice(), false)
                        .unwrap();
                    *buf_range = 0..BUF_SIZE;
                    Ok(())
                }
                Err(_) => Err(()),
            }
        }

        Ok(move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
            let mut already_failed = false;
            for frame in data.chunks_mut(2) {
                if Range::<usize>::is_empty(&buf_range) && !already_failed {
                    match fill_buf(&mut packets, &mut decoder, &mut buf, &mut buf_range) {
                        Ok(()) => (),
                        Err(()) => already_failed = true,
                    }
                }
                let value = if !Range::<usize>::is_empty(&buf_range) {
                    let value = 0.7 * buf[buf_range.start];
                    buf_range.start += 1;
                    value
                } else {
                    // We have no data to decode. Fill with silence.
                    0.0
                };

                for sample in frame.iter_mut() {
                    *sample = value;
                }
            }
        })
    }
}

fn set_ctrlc_handler() -> cancel::Token {
    let cancel_token = cancel::Token::new();
    ctrlc::set_handler({
        let cancel_token = cancel_token.clone();
        move || {
            log::info!("Caught Ctrl+C; shutting down");
            cancel_token.cancel();
        }
    })
    .unwrap();
    cancel_token
}
