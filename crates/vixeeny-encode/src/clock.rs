// SPDX-License-Identifier: GPL-3.0-or-later
//! The master clock side of the pipeline (plan 4.4): captured frames carry the OS monotonic time;
//! the output is a constant frame rate. Late frames are repeated, early ones dropped, and a pause
//! removes its duration from the media time so the file has no hole. Pure, so it is tested with
//! made-up timelines (CA-REC-3).

/// A frame rate as a fraction (`60/1`, `30000/1001`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fps {
    pub num: u32,
    pub den: u32,
}

impl Fps {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    pub const fn whole(fps: u32) -> Self {
        Self { num: fps, den: 1 }
    }

    /// Output slot of media time `ns` (nearest slot).
    fn slot(self, ns: i64) -> i64 {
        let num = i128::from(self.num);
        let den = i128::from(self.den);
        // ns * num / (den * 1e9), rounded to nearest.
        let scaled = i128::from(ns) * num * 2 + den * 1_000_000_000;
        (scaled.div_euclid(den * 2_000_000_000)) as i64
    }

    /// Media time of the start of slot `index`.
    #[cfg(test)]
    fn time_of(self, index: u64) -> i64 {
        (i128::from(index) * i128::from(self.den) * 1_000_000_000 / i128::from(self.num)) as i64
    }
}

/// What to put in the output at a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emit {
    /// The frame that was just delivered.
    New(u64),
    /// The previous frame again (nothing newer arrived in time).
    Repeat(u64),
}

impl Emit {
    pub const fn index(self) -> u64 {
        match self {
            Self::New(i) | Self::Repeat(i) => i,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cfr {
    fps: Fps,
    start: Option<i64>,
    paused_total: i64,
    paused_at: Option<i64>,
    next: u64,
    have_previous: bool,
    pub dropped: u64,
    pub repeated: u64,
}

impl Cfr {
    pub fn new(fps: Fps) -> Self {
        Self {
            fps,
            start: None,
            paused_total: 0,
            paused_at: None,
            next: 0,
            have_previous: false,
            dropped: 0,
            repeated: 0,
        }
    }

    pub fn fps(&self) -> Fps {
        self.fps
    }

    /// Index the next emitted frame will get = frames emitted so far.
    pub fn emitted(&self) -> u64 {
        self.next
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    fn media_time(&self, t: i64) -> i64 {
        t - self.start.unwrap_or(t) - self.paused_total
    }

    fn slot(&self, t: i64) -> u64 {
        self.fps.slot(self.media_time(t)).max(0) as u64
    }

    fn repeats_until(&mut self, end: u64, out: &mut Vec<Emit>) {
        if !self.have_previous {
            return;
        }
        while self.next < end {
            out.push(Emit::Repeat(self.next));
            self.next += 1;
            self.repeated += 1;
        }
    }

    /// A frame captured at `t` (nanoseconds, monotonic).
    pub fn on_frame(&mut self, t: i64) -> Vec<Emit> {
        let mut out = Vec::new();
        if self.is_paused() {
            return out;
        }
        if self.start.is_none() {
            self.start = Some(t);
        }
        let slot = self.slot(t);
        if slot < self.next {
            self.dropped += 1; // arrived early: its slot is already filled
            return out;
        }
        self.repeats_until(slot, &mut out);
        // Without a previous frame the first slot is simply this one.
        let index = slot.max(self.next);
        out.push(Emit::New(index));
        self.next = index + 1;
        self.have_previous = true;
        out
    }

    /// Time passes with no new frame (a static screen): repeats fill the slots that are over.
    pub fn on_tick(&mut self, t: i64) -> Vec<Emit> {
        let mut out = Vec::new();
        if self.is_paused() || self.start.is_none() {
            return out;
        }
        let slot = self.slot(t);
        self.repeats_until(slot, &mut out);
        out
    }

    /// Pauses at `t`; the slots up to `t` are filled first.
    pub fn pause(&mut self, t: i64) -> Vec<Emit> {
        let out = self.on_tick(t);
        if self.paused_at.is_none() && self.start.is_some() {
            self.paused_at = Some(t);
        }
        out
    }

    pub fn resume(&mut self, t: i64) {
        if let Some(at) = self.paused_at.take() {
            self.paused_total += t - at;
        }
    }

    /// Stops at `t`: fills up to the stop time (or up to the pause time, when paused).
    pub fn finish(&mut self, t: i64) -> Vec<Emit> {
        let end = self.paused_at.unwrap_or(t);
        let mut out = Vec::new();
        if self.start.is_some() {
            let slot = self.slot(end);
            self.repeats_until(slot, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    /// Feeds frames at the given times; returns every emit in order.
    fn feed(cfr: &mut Cfr, times: &[i64]) -> Vec<Emit> {
        times.iter().flat_map(|t| cfr.on_frame(*t)).collect()
    }

    fn indices(e: &[Emit]) -> Vec<u64> {
        e.iter().map(|e| e.index()).collect()
    }

    #[test]
    fn a_regular_stream_maps_one_to_one() {
        let mut c = Cfr::new(Fps::whole(50)); // 20 ms
        let times: Vec<i64> = (0..10).map(|i| 1_000 * MS + i * 20 * MS).collect();
        let e = feed(&mut c, &times);
        assert_eq!(indices(&e), (0..10).collect::<Vec<_>>());
        assert!(e.iter().all(|e| matches!(e, Emit::New(_))));
    }

    #[test]
    fn jitter_stays_in_its_slot() {
        let mut c = Cfr::new(Fps::whole(50));
        let times = [0, 21, 38, 62, 79, 101].map(|ms| ms * MS);
        assert_eq!(indices(&feed(&mut c, &times)), [0, 1, 2, 3, 4, 5]);
        assert_eq!((c.dropped, c.repeated), (0, 0));
    }

    #[test]
    fn a_gap_repeats_the_previous_frame() {
        let mut c = Cfr::new(Fps::whole(50));
        let e = feed(&mut c, &[0, 100 * MS]);
        assert_eq!(
            e,
            [
                Emit::New(0),
                Emit::Repeat(1),
                Emit::Repeat(2),
                Emit::Repeat(3),
                Emit::Repeat(4),
                Emit::New(5)
            ]
        );
        assert_eq!(c.repeated, 4);
    }

    #[test]
    fn early_frames_are_dropped() {
        let mut c = Cfr::new(Fps::whole(50));
        // Two frames in the same slot: the second is dropped; a burst of three too.
        let e = feed(
            &mut c,
            &[0, 5 * MS, 20 * MS, 22 * MS, 25 * MS, 40 * MS].map(|x| x),
        );
        assert_eq!(indices(&e), [0, 1, 2]);
        assert_eq!(c.dropped, 3);
    }

    #[test]
    fn ticks_fill_a_static_screen() {
        let mut c = Cfr::new(Fps::whole(50));
        c.on_frame(0);
        let e = c.on_tick(100 * MS);
        assert_eq!(indices(&e), [1, 2, 3, 4]);
        // Nothing twice.
        assert!(c.on_tick(100 * MS).is_empty());
        assert_eq!(c.emitted(), 5);
        // A tick before the first frame does nothing.
        assert!(Cfr::new(Fps::whole(50)).on_tick(5_000 * MS).is_empty());
    }

    #[test]
    fn pause_removes_its_duration_without_a_hole() {
        let mut c = Cfr::new(Fps::whole(50));
        let mut all = feed(&mut c, &(0..50).map(|i| i * 20 * MS).collect::<Vec<_>>()); // 1 s
        all.extend(c.pause(1_000 * MS));
        assert!(c.is_paused());
        // Frames during the pause are ignored.
        assert!(c.on_frame(1_500 * MS).is_empty());
        assert!(c.on_tick(2_000 * MS).is_empty());
        c.resume(3_000 * MS); // paused for 2 s
        all.extend(feed(
            &mut c,
            &(0..50)
                .map(|i| 3_000 * MS + i * 20 * MS)
                .collect::<Vec<_>>(),
        ));
        all.extend(c.finish(4_000 * MS));
        // 2 s of recording = 100 frames, contiguous indices.
        assert_eq!(indices(&all), (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn stopping_while_paused_ends_at_the_pause() {
        let mut c = Cfr::new(Fps::whole(50));
        feed(&mut c, &[0, 20 * MS]);
        c.pause(40 * MS);
        let e = c.finish(10_000 * MS);
        assert!(e.is_empty());
        assert_eq!(c.emitted(), 2);
    }

    #[test]
    fn finish_fills_to_the_stop_time() {
        let mut c = Cfr::new(Fps::whole(50));
        c.on_frame(0);
        assert_eq!(indices(&c.finish(100 * MS)), [1, 2, 3, 4]);
    }

    #[test]
    fn fractional_rates_do_not_drift() {
        // 30000/1001 over one hour: slots stay within one frame of the media time.
        let fps = Fps::new(30_000, 1001);
        let mut c = Cfr::new(fps);
        let hour = 3_600_000 * MS;
        let n = (hour as i128 * 30_000 / (1001 * 1_000_000_000)) as u64;
        for i in 0..n {
            let e = c.on_frame(fps.time_of(i) + 3 * MS);
            assert_eq!(e, [Emit::New(i)], "frame {i}");
        }
        assert_eq!(c.emitted(), n);
        assert_eq!((c.dropped, c.repeated), (0, 0));
    }

    #[test]
    fn a_clock_started_late_still_begins_at_zero() {
        let mut c = Cfr::new(Fps::whole(60));
        assert_eq!(c.on_frame(987_654_321_000), [Emit::New(0)]);
    }
}
