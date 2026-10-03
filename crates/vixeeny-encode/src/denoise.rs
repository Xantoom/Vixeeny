// SPDX-License-Identifier: GPL-3.0-or-later
//! Microphone noise reduction (plan 5.10): FFmpeg's `afftdn` (FFT denoiser) in a filter graph.
//! Stereo 48 kHz f32 in, the same out. The filter keeps a little audio inside (a few tens of
//! milliseconds), so `process` may return fewer or more samples than it was given; the caller
//! places the output on its own timeline.

use ffmpeg_next::format::Sample;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::{ChannelLayout, Error, filter, frame};

use crate::audio::SAMPLE_RATE;

pub struct Denoiser {
    graph: filter::Graph,
    next_pts: i64,
}

impl Denoiser {
    /// `reduction_db`: how much noise to remove (afftdn `nr`, 0.01 to 97). `floor_db`: the noise
    /// level to start from (afftdn `nf`, -80 to -20 dBFS); it is then tracked while the
    /// microphone is quiet (`tn=1`).
    pub fn new(reduction_db: f32, floor_db: f32) -> Result<Self, Error> {
        let mut graph = filter::Graph::new();
        let args = format!(
            "time_base=1/{SAMPLE_RATE}:sample_rate={SAMPLE_RATE}:sample_fmt=flt:channel_layout=stereo"
        );
        let abuffer = filter::find("abuffer").ok_or(Error::FilterNotFound)?;
        let sink = filter::find("abuffersink").ok_or(Error::FilterNotFound)?;
        graph.add(&abuffer, "in", &args)?;
        graph.add(&sink, "out", "")?;
        let nr = reduction_db.clamp(0.01, 97.0);
        let nf = floor_db.clamp(-80.0, -20.0);
        graph.output("in", 0)?.input("out", 0)?.parse(&format!(
            "afftdn=nr={nr}:nf={nf}:tn=1,aformat=sample_fmts=flt:channel_layouts=stereo"
        ))?;
        graph.validate()?;
        Ok(Self { graph, next_pts: 0 })
    }

    /// Filters interleaved stereo samples.
    pub fn process(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, Error> {
        let frames = interleaved.len() / 2;
        if frames == 0 {
            return Ok(Vec::new());
        }
        let mut input = frame::Audio::new(
            Sample::F32(SampleType::Packed),
            frames,
            ChannelLayout::STEREO,
        );
        input.set_rate(SAMPLE_RATE);
        input.set_pts(Some(self.next_pts));
        self.next_pts += frames as i64;
        let bytes = frames * 2 * size_of::<f32>();
        for (dst, sample) in input.data_mut(0)[..bytes]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(interleaved)
        {
            dst.copy_from_slice(&sample.to_ne_bytes());
        }
        self.graph
            .get("in")
            .ok_or(Error::FilterNotFound)?
            .source()
            .add(&input)?;
        let mut out = Vec::with_capacity(interleaved.len());
        let mut filtered = frame::Audio::empty();
        loop {
            let mut sink = self.graph.get("out").ok_or(Error::FilterNotFound)?;
            match sink.sink().frame(&mut filtered) {
                Ok(()) => {
                    let n = filtered.samples() * 2 * size_of::<f32>();
                    out.extend(
                        filtered.data(0)[..n]
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|b| f32::from_ne_bytes(*b)),
                    );
                }
                // EAGAIN (needs more input) or EOF: nothing more for now.
                Err(_) => break,
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic white-ish noise and a sine, interleaved stereo.
    fn signal(frames: usize, tone: bool) -> Vec<f32> {
        let mut seed = 0x1234_5678_u32;
        (0..frames)
            .flat_map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.01;
                let tone = if tone {
                    0.5 * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0).sin()
                } else {
                    0.0
                };
                [noise + tone, noise + tone]
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    #[test]
    fn steady_noise_is_reduced_and_the_tone_survives() {
        let mut denoiser = Denoiser::new(24.0, -45.0).unwrap_or_else(|e| panic!("graph: {e}"));
        // Two seconds of noise let the filter learn it; 10 ms blocks like a capture callback.
        let noise = signal(96_000, false);
        let mut quiet = Vec::new();
        for block in noise.chunks(960) {
            quiet.extend(denoiser.process(block).unwrap_or_default());
        }
        let tail = &quiet[quiet.len() - 19_200..];
        assert!(
            rms(tail) < rms(&noise) * 0.8,
            "noise {} -> {}",
            rms(&noise),
            rms(tail)
        );

        let mut speech = Vec::new();
        for block in signal(48_000, true).chunks(960) {
            speech.extend(denoiser.process(block).unwrap_or_default());
        }
        assert!(rms(&speech[speech.len() / 2..]) > 0.25, "the tone is kept");
        assert!(speech.iter().all(|s| s.is_finite()));
    }
}
