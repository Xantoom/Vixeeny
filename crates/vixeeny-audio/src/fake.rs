// SPDX-License-Identifier: GPL-3.0-or-later
//! A source for tests: a tone, delivered in chunks like a real device (jittery arrival, clock
//! that can drift), driven by the caller so that an hour of audio runs in a moment.

use crate::{AudioChunk, AudioError, AudioSink, AudioSource, CHANNELS, SAMPLE_RATE};

/// Generates `frequency` Hz beeps: `beep_ms` of tone every `period_ms`, the rest silence.
pub struct FakeAudioSource {
    pub frequency: f32,
    pub amplitude: f32,
    pub beep_ms: u32,
    pub period_ms: u32,
    /// Device clock speed relative to the master clock (1.0 = exact).
    pub drift: f64,
    produced: u64,
    sink: Option<Box<dyn AudioSink>>,
}

impl FakeAudioSource {
    pub fn new(frequency: f32) -> Self {
        Self {
            frequency,
            amplitude: 0.5,
            beep_ms: 100,
            period_ms: 1_000,
            drift: 1.0,
            produced: 0,
            sink: None,
        }
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
        AudioChunk { time_ns, samples }
    }

    /// Makes a chunk and hands it to the sink given to `start`.
    pub fn deliver(&mut self, frames: usize) {
        let chunk = self.next_chunk(frames);
        if let Some(sink) = &mut self.sink {
            sink.on_audio(chunk);
        }
    }
}

impl AudioSource for FakeAudioSource {
    fn start(&mut self, sink: Box<dyn AudioSink>) -> Result<(), AudioError> {
        self.sink = Some(sink);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.sink = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Collect(Arc<Mutex<Vec<AudioChunk>>>);
    impl AudioSink for Collect {
        fn on_audio(&mut self, chunk: AudioChunk) {
            self.0.lock().unwrap().push(chunk);
        }
        fn on_event(&mut self, _: crate::SourceEvent) {}
    }

    #[test]
    fn beeps_are_where_the_timeline_says() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let mut s = FakeAudioSource::new(1000.0);
        s.start(Box::new(Collect(got.clone()))).unwrap();
        for _ in 0..200 {
            s.deliver(480); // 2 s
        }
        let chunks = got.lock().unwrap();
        assert_eq!(chunks[0].time_ns, 0);
        assert_eq!(chunks[1].time_ns, 10_000_000);
        let loud = |c: &AudioChunk| c.samples.iter().any(|v| v.abs() > 0.1);
        assert!(loud(&chunks[0])); // 0 – 10 ms: inside the first beep
        assert!(!loud(&chunks[50])); // 500 ms: silence
        assert!(loud(&chunks[100])); // 1 000 ms: second beep
    }

    #[test]
    fn a_fast_clock_dates_chunks_earlier() {
        let mut s = FakeAudioSource::new(440.0);
        s.drift = 1.001;
        s.next_chunk(48_000);
        let c = s.next_chunk(480);
        assert!(c.time_ns < 1_000_000_000);
    }
}
