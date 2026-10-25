// SPDX-License-Identifier: GPL-3.0-or-later
//! A source for tests: a tone in chunks dated like a real device's (its clock can drift), driven by the caller so that
//! an hour of audio runs in a moment.

use crate::{AudioChunk, CHANNELS, SAMPLE_RATE};

/// Generates `frequency` Hz beeps: `beep_ms` of tone every `period_ms`, the rest silence.
pub struct FakeAudioSource {
    pub frequency: f32,
    pub amplitude: f32,
    pub beep_ms: u32,
    pub period_ms: u32,
    /// Device clock speed relative to the master clock (1.0 = exact).
    pub drift: f64,
    produced: u64,
}

impl FakeAudioSource {
    pub fn new(frequency: f32) -> Self {
        Self { frequency, amplitude: 0.5, beep_ms: 100, period_ms: 1_000, drift: 1.0, produced: 0 }
    }

    /// One chunk of `frames` frames, dated on the master clock.
    pub fn next_chunk(&mut self, frames: usize) -> AudioChunk {
        let mut samples = Vec::with_capacity(frames * CHANNELS);
        for i in 0..frames {
            let n = self.produced + i as u64;
            let ms = n * 1000 / u64::from(SAMPLE_RATE);
            let on = ms % u64::from(self.period_ms) < u64::from(self.beep_ms);
            let v = if on {
                let phase = n as f32 / SAMPLE_RATE as f32 * self.frequency * std::f32::consts::TAU;
                phase.sin() * self.amplitude
            } else {
                0.0
            };
            samples.extend_from_slice(&[v, v]);
        }
        let time_ns = (self.produced as f64 / f64::from(SAMPLE_RATE) / self.drift * 1e9) as i64;
        self.produced += frames as u64;
        AudioChunk::stereo(time_ns, samples)
    }
}
