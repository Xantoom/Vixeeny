// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio capture and mixing for recordings (plan 5.10).
//!
//! Every source delivers 48 kHz, 32-bit float samples in 2, 6 or 8 channels (the OS converts the
//! sample rate; the channel count is the device's own, see [`layout`]), each chunk dated
//! on the master clock (the OS monotonic clock, nanoseconds, same as video). The [`Mixer`]
//! places the chunks of one track on a continuous timeline, fills gaps with silence and corrects
//! clock drift, so the encoder gets gapless audio that stays in sync with the video.

mod fake;
mod layout;
mod mixer;
mod routing;
mod spec;
mod wasapi;

pub use fake::FakeAudioSource;
pub use layout::{convert as convert_layout, track_channels};
pub use mixer::{Block, Mixer};
pub use routing::{
    CAPTURE_VOLUME, MIC_VOLUME, TrackMember, TrackPlan, assign_channels, plan_tracks,
};
pub use spec::{SourceKind, SourceSpec, parse_sources};
pub use wasapi::{
    AppInfo, DeviceInfo, WasapiSource, list_applications, list_microphones, list_outputs,
    source_channels,
};

/// Samples per second, per channel.
pub const SAMPLE_RATE: u32 = 48_000;
/// Channels of a stereo chunk, the default.
pub const CHANNELS: usize = 2;
/// The most channels a track can have (7.1).
pub const MAX_CHANNELS: usize = 8;

/// A run of interleaved samples.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    /// Master-clock time of the first sample, nanoseconds.
    pub time_ns: i64,
    /// Interleaved channels: 2, 6 or 8.
    pub channels: usize,
    /// `frames × channels` samples in -1…1.
    pub samples: Vec<f32>,
}

impl AudioChunk {
    pub fn stereo(time_ns: i64, samples: Vec<f32>) -> Self {
        Self {
            time_ns,
            channels: CHANNELS,
            samples,
        }
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

/// Things a source reports besides samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceEvent {
    /// The device or application went away: the track is silent until it comes back.
    Lost(String),
    /// A lost source is delivering again.
    Back,
}

/// Where a source sends what it captures. Called from the source's own thread.
pub trait AudioSink: Send {
    fn on_audio(&mut self, chunk: AudioChunk);
    fn on_event(&mut self, event: SourceEvent);
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio source unavailable: {0}")]
    Unavailable(String),
    #[error("audio: {0}")]
    Os(String),
    #[error("not supported on this platform")]
    Unsupported,
}

pub trait AudioSource: Send {
    /// Starts delivering to `sink` from a thread of the source's own.
    fn start(&mut self, sink: Box<dyn AudioSink>) -> Result<(), AudioError>;
    /// Stops and joins; no call to the sink happens afterwards.
    fn stop(&mut self) -> Result<(), AudioError>;
}
