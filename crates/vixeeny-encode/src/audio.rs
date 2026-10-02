// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio encoders of the recorder (plan 5.10): AAC (Media Foundation / AudioToolbox / native),
//! Opus, FLAC, PCM. Input is 48 kHz stereo 32-bit float; swresample converts to whatever the
//! encoder wants, and a FIFO cuts the frame size it asks for.

use ffmpeg_next::format::Sample;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::software::resampling;
use ffmpeg_next::{ChannelLayout, Dictionary, Packet, Rational, codec, encoder, frame};

use crate::recorder::{OutputContainer, RecordError};

/// The sample rate and layout of the recorder's audio input.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Aac,
    Opus,
    Flac,
    Pcm16,
    Pcm24,
}

impl AudioCodec {
    /// `auto` = AAC in MP4, Opus in MKV and WebM (plan 5.10).
    pub fn from_setting(name: &str, container: OutputContainer) -> Option<Self> {
        Some(match name {
            "auto" => match container {
                OutputContainer::Mp4Hybrid | OutputContainer::Mp4Fragmented => Self::Aac,
                OutputContainer::Mkv | OutputContainer::WebM => Self::Opus,
            },
            "aac" => Self::Aac,
            "opus" => Self::Opus,
            "flac" => Self::Flac,
            "pcm" | "pcm16" => Self::Pcm16,
            "pcm24" => Self::Pcm24,
            _ => return None,
        })
    }

    /// FFmpeg encoder names to try, best first.
    fn encoder_names(self) -> &'static [&'static str] {
        match self {
            Self::Aac if cfg!(windows) => &["aac_mf", "aac"],
            Self::Aac if cfg!(target_os = "macos") => &["aac_at", "aac"],
            Self::Aac => &["aac"],
            Self::Opus => &["libopus"],
            Self::Flac => &["flac"],
            Self::Pcm16 => &["pcm_s16le"],
            Self::Pcm24 => &["pcm_s24le"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioTrackConfig {
    /// Track name in the file (the sources it holds).
    pub title: String,
    pub codec: AudioCodec,
    pub bitrate_kbps: u32,
    /// Variable bit rate where the encoder offers it (Opus); AAC stays constant.
    pub vbr: bool,
}

/// One track's encoder, fed with interleaved f32 samples.
pub(crate) struct AudioEncoder {
    pub(crate) encoder: encoder::Audio,
    pub(crate) codec: ffmpeg_next::Codec,
    resampler: resampling::Context,
    fifo: Vec<f32>,
    frame_samples: usize,
    /// Index (in samples) of the next frame to encode.
    next_pts: i64,
}

impl AudioEncoder {
    pub(crate) fn open(cfg: &AudioTrackConfig, global_header: bool) -> Result<Self, RecordError> {
        let codec = cfg
            .codec
            .encoder_names()
            .iter()
            .find_map(|n| encoder::find_by_name(n))
            .ok_or_else(|| {
                RecordError::Config(format!("no encoder for {:?} in this FFmpeg", cfg.codec))
            })?;
        let mut ctx = codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        let format = codec
            .audio()
            .ok()
            .and_then(|a| a.formats())
            .and_then(|mut f| f.next())
            .ok_or_else(|| RecordError::Config("audio encoder without sample formats".into()))?;
        ctx.set_rate(SAMPLE_RATE as i32);
        ctx.set_channel_layout(ChannelLayout::STEREO);
        ctx.set_format(format);
        ctx.set_time_base(Rational(1, SAMPLE_RATE as i32));
        if !matches!(
            cfg.codec,
            AudioCodec::Flac | AudioCodec::Pcm16 | AudioCodec::Pcm24
        ) {
            ctx.set_bit_rate(cfg.bitrate_kbps as usize * 1000);
        }
        if global_header {
            ctx.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let mut options = Dictionary::new();
        match cfg.codec {
            AudioCodec::Opus => options.set("vbr", if cfg.vbr { "on" } else { "off" }),
            AudioCodec::Flac => options.set("compression_level", "5"),
            _ => {}
        }
        let opened = ctx.open_with(options)?;
        let resampler = resampling::Context::get(
            Sample::F32(SampleType::Packed),
            ChannelLayout::STEREO,
            SAMPLE_RATE,
            format,
            ChannelLayout::STEREO,
            SAMPLE_RATE,
        )?;
        let frame_samples = match opened.frame_size() as usize {
            0 => 960,
            n => n,
        };
        Ok(Self {
            encoder: opened,
            codec,
            resampler,
            fifo: Vec::new(),
            frame_samples,
            next_pts: 0,
        })
    }

    /// Queues samples and returns the packets that became ready.
    pub(crate) fn push(&mut self, samples: &[f32]) -> Result<Vec<Packet>, RecordError> {
        self.fifo.extend_from_slice(samples);
        let mut packets = Vec::new();
        let per_frame = self.frame_samples * CHANNELS;
        while self.fifo.len() >= per_frame {
            let chunk: Vec<f32> = self.fifo.drain(..per_frame).collect();
            self.encode(&chunk, &mut packets)?;
        }
        Ok(packets)
    }

    /// Encodes what is left (padded with silence to a whole frame) and drains the encoder.
    pub(crate) fn finish(&mut self) -> Result<Vec<Packet>, RecordError> {
        let mut packets = Vec::new();
        if !self.fifo.is_empty() {
            let mut rest = std::mem::take(&mut self.fifo);
            rest.resize(self.frame_samples * CHANNELS, 0.0);
            self.encode(&rest, &mut packets)?;
        }
        self.encoder.send_eof()?;
        self.receive(&mut packets);
        Ok(packets)
    }

    fn encode(&mut self, interleaved: &[f32], out: &mut Vec<Packet>) -> Result<(), RecordError> {
        let frames = interleaved.len() / CHANNELS;
        let mut input = frame::Audio::new(
            Sample::F32(SampleType::Packed),
            frames,
            ChannelLayout::STEREO,
        );
        input.set_rate(SAMPLE_RATE);
        let bytes: &[u8] = bytemuck_cast(interleaved);
        input.data_mut(0)[..bytes.len()].copy_from_slice(bytes);
        let mut converted = frame::Audio::empty();
        self.resampler.run(&input, &mut converted)?;
        converted.set_pts(Some(self.next_pts));
        self.next_pts += frames as i64;
        self.encoder.send_frame(&converted)?;
        self.receive(out);
        Ok(())
    }

    fn receive(&mut self, out: &mut Vec<Packet>) {
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            out.push(std::mem::replace(&mut packet, Packet::empty()));
        }
    }
}

/// f32 samples as bytes (native endian, which is what `Sample::F32` means).
fn bytemuck_cast(samples: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and no invalid bit patterns as bytes; the length is exact.
    unsafe { std::slice::from_raw_parts(samples.as_ptr().cast(), std::mem::size_of_val(samples)) }
}
