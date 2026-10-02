// SPDX-License-Identifier: GPL-3.0-or-later
//! One track: the sources assigned to it, placed on a continuous timeline.
//!
//! WASAPI delivers nothing while an application is silent, and device clocks drift from the
//! master clock, so the mixer is driven by *time*: [`Mixer::drain`] emits fixed blocks up to
//! `now − latency`, taking what each source delivered by then and silence for the rest. A chunk
//! continues the previous one when its timestamp agrees to within [`RESYNC_FRAMES`]; otherwise it
//! is placed by its timestamp (a gap becomes silence, an overlap overwrites), which bounds the
//! error against the master clock whatever the drift.

use crate::{AudioChunk, CHANNELS, SAMPLE_RATE};

/// 20 ms.
const BLOCK_FRAMES: u64 = 960;
/// Data older than this is final (sources deliver within a few tens of milliseconds).
const LATENCY_NS: i64 = 150_000_000;
/// 10 ms.
const RESYNC_FRAMES: i64 = 480;

/// A run of mixed, gapless samples: the next ones of the track.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub samples: Vec<f32>,
}

impl Block {
    pub fn frames(&self) -> usize {
        self.samples.len() / CHANNELS
    }
}

struct Source {
    volume: f32,
    /// Interleaved samples from track index `Mixer::next` on.
    buf: Vec<f32>,
    /// Track index just after the last chunk, to recognise a continuation.
    expected: Option<u64>,
}

#[derive(Clone, Copy)]
enum State {
    Running,
    /// Paused at `at_ns`; blocks up to track index `end` are still to be emitted.
    Pausing { end: u64, at_ns: i64 },
    Paused,
}

pub struct Mixer {
    sources: Vec<Source>,
    /// Master time and track index at which the current stretch (since the last resume) began.
    epoch_ns: i64,
    epoch_idx: u64,
    /// Track index of the next sample to emit.
    next: u64,
    state: State,
}

impl Mixer {
    /// One source per entry of `volumes` (1.0 = unchanged). The track starts at `origin_ns`.
    pub fn new(volumes: &[f32], origin_ns: i64) -> Self {
        Self {
            sources: volumes
                .iter()
                .map(|&volume| Source {
                    volume,
                    buf: Vec::new(),
                    expected: None,
                })
                .collect(),
            epoch_ns: origin_ns,
            epoch_idx: 0,
            next: 0,
            state: State::Running,
        }
    }

    /// Track index of a master time (may be before `next`: late data).
    fn raw_idx(&self, ns: i64) -> i64 {
        let frames = i128::from(ns - self.epoch_ns) * i128::from(SAMPLE_RATE);
        let frames = (frames + 500_000_000).div_euclid(1_000_000_000);
        (i128::from(self.epoch_idx) + frames) as i64
    }

    /// Track index of a master time, never behind what was emitted.
    fn idx_of(&self, ns: i64) -> u64 {
        self.raw_idx(ns).max(self.next as i64) as u64
    }

    /// Places a chunk of source `source`.
    pub fn push(&mut self, source: usize, chunk: &AudioChunk) {
        match self.state {
            State::Paused => return,
            State::Pausing { at_ns, .. } if chunk.time_ns >= at_ns => return,
            _ => {}
        }
        let frames = chunk.frames() as u64;
        // Stale: entirely from before the last resume.
        let duration_ns = (frames * 1_000_000_000 / u64::from(SAMPLE_RATE)) as i64;
        if chunk.time_ns + duration_ns <= self.epoch_ns {
            return;
        }
        let mut at = self.raw_idx(chunk.time_ns);
        let next = self.next;
        let first_of_stretch = self.sources.get(source).is_some_and(|s| s.expected.is_none());
        if first_of_stretch {
            // A stream that starts a few ms before the stretch keeps its first samples.
            at = at.max(self.epoch_idx as i64);
        }
        let Some(s) = self.sources.get_mut(source) else {
            return;
        };
        let mut start = match s.expected {
            Some(e) if (at - e as i64).abs() <= RESYNC_FRAMES => e as i64,
            _ => at,
        };
        s.expected = Some((start + frames as i64).max(0) as u64);
        let mut data = &chunk.samples[..frames as usize * CHANNELS];
        if start < next as i64 {
            // Already emitted: keep only what is still ahead.
            let skip = next as i64 - start;
            if skip >= frames as i64 {
                return;
            }
            data = &data[skip as usize * CHANNELS..];
            start = next as i64;
        }
        let from = (start - next as i64) as usize * CHANNELS;
        if s.buf.len() < from + data.len() {
            s.buf.resize(from + data.len(), 0.0);
        }
        s.buf[from..from + data.len()].copy_from_slice(data);
    }

    fn emit_until(&mut self, limit: u64, partial: bool, out: &mut Vec<Block>) {
        while self.next < limit && (partial || self.next + BLOCK_FRAMES <= limit) {
            let len = BLOCK_FRAMES.min(limit - self.next) as usize;
            let mut samples = vec![0.0f32; len * CHANNELS];
            for s in &mut self.sources {
                let take = (len * CHANNELS).min(s.buf.len());
                for (acc, v) in samples.iter_mut().zip(&s.buf[..take]) {
                    *acc += v * s.volume;
                }
                s.buf.drain(..take);
            }
            for v in &mut samples {
                *v = v.clamp(-1.0, 1.0);
            }
            self.next += len as u64;
            out.push(Block { samples });
        }
    }

    /// The blocks that are final at master time `now`.
    pub fn drain(&mut self, now: i64) -> Vec<Block> {
        let mut out = Vec::new();
        let settled = now - LATENCY_NS;
        match self.state {
            State::Running => {
                if settled > self.epoch_ns {
                    let limit = self.idx_of(settled);
                    self.emit_until(limit, false, &mut out);
                }
            }
            State::Pausing { end, at_ns } => {
                if settled >= at_ns {
                    self.emit_until(end, true, &mut out);
                    self.state = State::Paused;
                } else if settled > self.epoch_ns {
                    let limit = self.idx_of(settled).min(end);
                    self.emit_until(limit, false, &mut out);
                }
            }
            State::Paused => {}
        }
        out
    }

    /// Stops the track at `t_ns`: what was captured up to then is still emitted by
    /// [`drain`](Self::drain), nothing after.
    pub fn pause(&mut self, t_ns: i64) {
        if matches!(self.state, State::Running) {
            let end = self.idx_of(t_ns);
            self.state = State::Pausing { end, at_ns: t_ns };
        }
    }

    /// Continues at `t_ns`: the track timeline carries on from where it stopped (no hole).
    pub fn resume(&mut self, t_ns: i64) -> Vec<Block> {
        let mut out = Vec::new();
        match self.state {
            State::Running => return out,
            State::Pausing { end, .. } => self.emit_until(end, true, &mut out),
            State::Paused => {}
        }
        for s in &mut self.sources {
            s.buf.clear();
            s.expected = None;
        }
        self.epoch_ns = t_ns;
        self.epoch_idx = self.next;
        self.state = State::Running;
        out
    }

    /// Everything up to `now`, whatever the latency.
    pub fn finish(&mut self, now: i64) -> Vec<Block> {
        let mut out = Vec::new();
        match self.state {
            State::Running => {
                let limit = self.idx_of(now);
                self.emit_until(limit, true, &mut out);
            }
            State::Pausing { end, .. } => {
                self.emit_until(end, true, &mut out);
                self.state = State::Paused;
            }
            State::Paused => {}
        }
        out
    }

    /// Samples emitted so far (per channel).
    pub fn emitted_frames(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;
    const S: i64 = 1_000_000_000;

    fn chunk(time_ns: i64, frames: usize, f: impl Fn(usize) -> f32) -> AudioChunk {
        let mut samples = Vec::with_capacity(frames * CHANNELS);
        for i in 0..frames {
            let v = f(i);
            samples.extend_from_slice(&[v, v]);
        }
        AudioChunk { time_ns, samples }
    }

    fn collect(blocks: &[Block]) -> Vec<f32> {
        blocks.iter().flat_map(|b| b.samples.iter().copied()).collect()
    }

    #[test]
    fn contiguous_chunks_with_timestamp_jitter_come_out_exact() {
        let mut m = Mixer::new(&[1.0], 0);
        let mut out = Vec::new();
        // 10 ms chunks of a ramp; the timestamps wobble by ±4 ms (below the resync threshold).
        for k in 0..300usize {
            let jitter = ((k * 7) % 9) as i64 - 4;
            let c = chunk(k as i64 * 10 * MS + jitter * MS, 480, |i| {
                ((k * 480 + i) as f32) / 1e6
            });
            m.push(0, &c);
            out.extend(m.drain(k as i64 * 10 * MS + 10 * MS));
        }
        out.extend(m.finish(3 * S));
        let all = collect(&out);
        assert_eq!(all.len(), 3 * 48_000 * CHANNELS);
        for (n, frame) in all.chunks(CHANNELS).enumerate() {
            assert_eq!(frame[0], (n as f32) / 1e6, "frame {n}");
        }
    }

    #[test]
    fn a_gap_becomes_silence_at_the_right_place() {
        let mut m = Mixer::new(&[1.0], 0);
        m.push(0, &chunk(0, 48_000, |_| 0.25)); // 0 – 1 s
        m.push(0, &chunk(2 * S, 48_000, |_| 0.25)); // 2 – 3 s (nothing in 1 – 2 s)
        let all = collect(&m.finish(3 * S));
        let at = |sec: f64| all[(sec * 48_000.0) as usize * CHANNELS];
        assert_eq!(at(0.5), 0.25);
        assert_eq!(at(1.5), 0.0);
        assert_eq!(at(2.5), 0.25);
        assert_eq!(all.len(), 3 * 48_000 * CHANNELS);
    }

    #[test]
    fn a_silent_source_still_yields_time_driven_blocks() {
        let mut m = Mixer::new(&[1.0], 0);
        let blocks = m.drain(2 * S);
        // 2 s minus the latency, in whole 20 ms blocks.
        let frames: usize = blocks.iter().map(Block::frames).sum();
        let want = ((2 * S - LATENCY_NS) as f64 / 1e9 * 48_000.0) as usize;
        assert!(want - frames < 960, "{frames} vs {want}");
        assert!(collect(&blocks).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn sources_are_mixed_with_their_volumes_and_clipped() {
        let mut m = Mixer::new(&[1.0, 0.5, 2.0], 0);
        m.push(0, &chunk(0, 4_800, |_| 0.2));
        m.push(1, &chunk(0, 4_800, |_| 0.4));
        let first = collect(&m.finish(100 * MS));
        assert!((first[0] - 0.4).abs() < 1e-6, "{}", first[0]); // 0.2 + 0.4 × 0.5

        let mut m = Mixer::new(&[1.0, 2.0], 0);
        m.push(0, &chunk(0, 4_800, |_| 0.6));
        m.push(1, &chunk(0, 4_800, |_| 0.6));
        assert_eq!(collect(&m.finish(100 * MS))[0], 1.0);
    }

    #[test]
    fn a_source_clock_running_fast_stays_within_20_ms_for_an_hour() {
        // CA-REC-4: the device clock gains 50 ppm; the track must not drift from the master clock.
        let mut m = Mixer::new(&[1.0], 0);
        let mut emitted = 0usize;
        let drift = 1.000_05f64;
        let mut produced = 0u64; // frames the device has made
        let mut t = 0i64;
        while t < 3_600 * S {
            // The device made 480 frames during this 10 ms of its own clock.
            let master = (produced as f64 / 48_000.0 / drift * 1e9) as i64;
            m.push(0, &chunk(master, 480, |_| 0.1));
            produced += 480;
            t += 10 * MS;
            emitted += m.drain(t).iter().map(Block::frames).sum::<usize>();
        }
        emitted += m.finish(t).iter().map(Block::frames).sum::<usize>();
        let expected = (t as f64 / 1e9 * 48_000.0) as i64;
        let error_ms = (emitted as i64 - expected).abs() as f64 / 48.0;
        assert!(error_ms < 20.0, "off by {error_ms:.2} ms");
    }

    #[test]
    fn pause_leaves_no_hole_and_drops_what_was_captured_meanwhile() {
        let mut m = Mixer::new(&[1.0], 0);
        let mut out = Vec::new();
        let mut t = 0i64;
        let step = |m: &mut Mixer, out: &mut Vec<Block>, upto: i64, t: &mut i64| {
            while *t < upto {
                m.push(0, &chunk(*t, 480, |_| 0.3));
                *t += 10 * MS;
                out.extend(m.drain(*t));
            }
        };
        step(&mut m, &mut out, S, &mut t);
        m.pause(S);
        step(&mut m, &mut out, 5 * S, &mut t); // captured while paused: ignored
        out.extend(m.resume(5 * S));
        step(&mut m, &mut out, 6 * S, &mut t);
        out.extend(m.finish(6 * S));
        let frames: usize = out.iter().map(Block::frames).sum();
        // 1 s before the pause + 1 s after the resume.
        assert!((frames as i64 - 96_000).abs() <= 2, "{frames}");
        assert!(collect(&out).iter().all(|v| (*v - 0.3).abs() < 1e-6));
    }

    #[test]
    fn late_data_for_an_emitted_block_is_dropped_not_misplaced() {
        let mut m = Mixer::new(&[1.0], 0);
        m.push(0, &chunk(0, 4_800, |_| 0.5));
        let early = collect(&m.drain(S));
        assert!(early.len() > 4_800 * CHANNELS);
        // A chunk dated 0 – 10 ms arrives after those blocks went out.
        m.push(0, &chunk(0, 480, |_| 0.9));
        let rest = collect(&m.finish(2 * S));
        assert!(rest.iter().all(|v| *v == 0.0));
    }
}
