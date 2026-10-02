// SPDX-License-Identifier: GPL-3.0-or-later
//! The replay buffer (plan 5.11): a ring of encoded packets in RAM, cut on key frames. Nothing is
//! re-encoded: a save remuxes the last seconds of packets into a file.
//!
//! Times are media time in audio samples (48 kHz), the same unit for video and audio.

use std::collections::VecDeque;
use std::sync::Arc;

use ffmpeg_next::packet::{Mut, Ref};
use ffmpeg_next::{Packet, ffi};

use crate::audio::SAMPLE_RATE;

/// A packet shared between the ring and a save in progress, without copying its data.
pub fn share(packet: &Packet) -> Packet {
    let mut copy = Packet::empty();
    // SAFETY: both are valid `AVPacket`s; the reference counts the data buffer, so the copy owns
    // its own fields (timestamps) over the same bytes.
    unsafe { ffi::av_packet_ref(copy.as_mut_ptr(), packet.as_ptr()) };
    copy
}

/// RAM the ring needs for `seconds` of replay at the given bitrates (video + all audio tracks, in
/// kbit/s). One more key frame interval is kept beyond the duration, so a save always starts on a
/// key frame at least `seconds` back.
pub fn estimate_ram_bytes(total_kbps: u32, seconds: u32, keyframe_seconds: f64) -> u64 {
    let span = f64::from(seconds) + keyframe_seconds;
    (f64::from(total_kbps) * 1000.0 / 8.0 * span) as u64
}

/// The allowed durations: 5 s to 20 min, in steps of 5 s.
pub fn clamp_seconds(seconds: u32) -> u32 {
    (seconds.clamp(5, 1200) + 2) / 5 * 5
}

struct Entry {
    /// Media time of the packet, in samples.
    t: i64,
    packet: Arc<Packet>,
}

/// A key frame and the packets up to the next one.
struct Gop {
    start: i64,
    packets: Vec<Entry>,
    bytes: usize,
}

/// What a save writes: everything from one key frame on.
pub struct Snapshot {
    /// Media time of the first (key) video packet.
    pub start: i64,
    /// `(time in samples, packet)` in order.
    pub video: Vec<(i64, Arc<Packet>)>,
    /// One list per audio track, packets from `start` on.
    pub audio: Vec<Vec<(i64, Arc<Packet>)>>,
}

pub struct Ring {
    /// How much to keep, in samples.
    keep: i64,
    gops: VecDeque<Gop>,
    audio: Vec<VecDeque<Entry>>,
    bytes: usize,
    newest: i64,
}

impl Ring {
    pub fn new(seconds: u32, tracks: usize) -> Self {
        Self {
            keep: i64::from(seconds) * i64::from(SAMPLE_RATE),
            gops: VecDeque::new(),
            audio: (0..tracks).map(|_| VecDeque::new()).collect(),
            bytes: 0,
            newest: i64::MIN,
        }
    }

    /// Compressed bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn push_video(&mut self, t: i64, packet: &Packet) {
        let size = packet.size();
        let entry = Entry {
            t,
            packet: Arc::new(share(packet)),
        };
        if packet.is_key() || self.gops.is_empty() {
            // Packets before the first key frame cannot be decoded on their own: not kept.
            if !packet.is_key() {
                return;
            }
            self.gops.push_back(Gop {
                start: t,
                packets: Vec::new(),
                bytes: 0,
            });
        }
        if let Some(gop) = self.gops.back_mut() {
            gop.packets.push(entry);
            gop.bytes += size;
        }
        self.bytes += size;
        self.newest = self.newest.max(t);
        self.trim();
    }

    pub fn push_audio(&mut self, track: usize, t: i64, packet: &Packet) {
        let Some(queue) = self.audio.get_mut(track) else {
            return;
        };
        self.bytes += packet.size();
        queue.push_back(Entry {
            t,
            packet: Arc::new(share(packet)),
        });
        self.newest = self.newest.max(t);
        self.trim();
    }

    /// Drops the oldest key frame interval while the next one is still old enough to start from.
    fn trim(&mut self) {
        while self.gops.len() >= 2 && self.newest - self.gops[1].start >= self.keep {
            if let Some(old) = self.gops.pop_front() {
                self.bytes -= old.bytes;
            }
        }
        let Some(start) = self.gops.front().map(|g| g.start) else {
            return;
        };
        for queue in &mut self.audio {
            // One packet of margin: the first one may straddle the cut.
            while queue.get(1).is_some_and(|p| p.t <= start) {
                if let Some(old) = queue.pop_front() {
                    self.bytes -= old.packet.size();
                }
            }
        }
    }

    /// The last `seconds` (at most what the ring holds), from a key frame. `None` while empty.
    pub fn snapshot(&self, seconds: u32) -> Option<Snapshot> {
        let want = i64::from(seconds) * i64::from(SAMPLE_RATE);
        // The oldest key frame that is no more than `want` back; else the newest one.
        let first = self
            .gops
            .iter()
            .position(|g| self.newest - g.start <= want)
            .unwrap_or(self.gops.len().checked_sub(1)?);
        let start = self.gops.get(first)?.start;
        let video = self
            .gops
            .iter()
            .skip(first)
            .flat_map(|g| g.packets.iter().map(|e| (e.t, Arc::clone(&e.packet))))
            .collect();
        let audio = self
            .audio
            .iter()
            .map(|q| {
                q.iter()
                    .filter(|e| e.t >= start)
                    .map(|e| (e.t, Arc::clone(&e.packet)))
                    .collect()
            })
            .collect();
        Some(Snapshot {
            start,
            video,
            audio,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(size: usize, key: bool) -> Packet {
        let mut p = Packet::copy(&vec![0; size]);
        if key {
            p.set_flags(ffmpeg_next::codec::packet::Flags::KEY);
        }
        p
    }

    const S: i64 = SAMPLE_RATE as i64;

    /// 30 fps video with a key frame every second, `kbps` in total.
    fn fill(ring: &mut Ring, seconds: i64, kbps: usize) {
        let frame = kbps * 1000 / 8 / 30;
        for i in 0..seconds * 30 {
            ring.push_video(i * S / 30, &packet(frame, i % 30 == 0));
        }
    }

    #[test]
    fn the_ring_keeps_the_duration_from_a_key_frame_and_forgets_the_rest() {
        let mut ring = Ring::new(10, 0);
        fill(&mut ring, 60, 800);
        let snap = ring.snapshot(10).unwrap();
        // Key frames every second: the save starts on one, 10 s before the newest packet (±1 s).
        assert_eq!(snap.start % S, 0);
        let span = (60 * S - S / 30 - snap.start) / S;
        assert!((9..=10).contains(&span), "{span}");
        assert!(snap.video.first().unwrap().1.is_key());
        // Asking for less gives less; asking for more than there is gives all there is.
        assert!(ring.snapshot(3).unwrap().start > snap.start);
        assert!(ring.snapshot(1200).unwrap().start <= snap.start);
    }

    #[test]
    fn ram_stays_within_ten_percent_of_the_estimate() {
        for seconds in [10, 30, 120] {
            let mut ring = Ring::new(seconds, 0);
            fill(&mut ring, i64::from(seconds) * 4, 20_000);
            let estimate = estimate_ram_bytes(20_000, seconds, 1.0) as f64;
            let used = ring.bytes() as f64;
            assert!(used <= estimate * 1.1, "{seconds}: {used} > {estimate}");
            assert!(
                used >= estimate * 0.9,
                "{seconds}: {used} too far under {estimate}"
            );
        }
    }

    #[test]
    fn audio_follows_the_video_cut() {
        let mut ring = Ring::new(5, 2);
        for i in 0..40 * 30 {
            let t = i * S / 30;
            ring.push_video(t, &packet(1000, i % 30 == 0));
            if i % 3 == 0 {
                for track in 0..2 {
                    ring.push_audio(track, t, &packet(100, true));
                }
            }
        }
        let snap = ring.snapshot(5).unwrap();
        assert_eq!(snap.audio.len(), 2);
        for track in &snap.audio {
            assert!(track.first().unwrap().0 >= snap.start);
            assert!(!track.is_empty());
        }
        // The bytes count includes the audio and shrinks with the trimming.
        assert!(ring.bytes() < 8 * 30 * 1000 + 8 * 10 * 100 * 2 + 5000);
    }

    #[test]
    fn nothing_to_save_before_the_first_key_frame() {
        let mut ring = Ring::new(10, 0);
        assert!(ring.snapshot(5).is_none());
        ring.push_video(0, &packet(10, false));
        assert!(ring.snapshot(5).is_none());
        ring.push_video(S / 30, &packet(10, true));
        assert_eq!(ring.snapshot(5).unwrap().video.len(), 1);
    }

    #[test]
    fn durations_are_whole_steps_of_five_seconds_between_5_s_and_20_min() {
        assert_eq!(clamp_seconds(0), 5);
        assert_eq!(clamp_seconds(30), 30);
        assert_eq!(clamp_seconds(32), 30);
        assert_eq!(clamp_seconds(33), 35);
        assert_eq!(clamp_seconds(99_999), 1200);
    }
}
