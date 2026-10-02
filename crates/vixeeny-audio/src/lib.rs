// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio capture and mixing for recordings (plan 5.10).
//!
//! Every source delivers 48 kHz, stereo, 32-bit float samples (the OS converts), each chunk dated
//! on the master clock (the OS monotonic clock, nanoseconds, same as video). The [`Mixer`]
//! places the chunks of one track on a continuous timeline, fills gaps with silence and corrects
//! clock drift, so the encoder gets gapless audio that stays in sync with the video.

mod fake;
mod mixer;
mod routing;
mod spec;
#[cfg(windows)]
mod wasapi;

pub use fake::FakeAudioSource;
pub use mixer::{Block, Mixer};
pub use routing::{TrackMember, TrackPlan, plan_tracks};
pub use spec::{SourceKind, SourceSpec, parse_sources};
#[cfg(windows)]
pub use wasapi::{AppInfo, DeviceInfo, WasapiSource, list_applications, list_microphones};

/// Samples per second, per channel.
pub const SAMPLE_RATE: u32 = 48_000;
/// Interleaved channels in every chunk.
pub const CHANNELS: usize = 2;

/// A run of interleaved samples.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    /// Master-clock time of the first sample, nanoseconds.
    pub time_ns: i64,
    /// `frames × CHANNELS` samples in -1…1.
    pub samples: Vec<f32>,
}

impl AudioChunk {
    pub fn frames(&self) -> usize {
        self.samples.len() / CHANNELS
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
