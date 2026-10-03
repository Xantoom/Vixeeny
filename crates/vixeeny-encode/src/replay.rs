// SPDX-License-Identifier: GPL-3.0-or-later
//! The replay buffer (plan 5.11): a ring of encoded packets, in RAM or in temporary files on
//! disk, cut on key frames. Nothing is re-encoded: a save remuxes the last seconds of packets into
//! a file.
//!
//! Times are media time in audio samples (48 kHz), the same unit for video and audio.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

/// Where the packets of a ring are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    Ram,
    /// Temporary files in this folder (a sub-folder per ring, removed with it).
    Disk(PathBuf),
}

/// A segment file holds this much before the next key frame starts a new one.
const SEGMENT_BYTES: u64 = 32 << 20;
/// Written to the file in blocks of about this size.
const WRITE_BLOCK: usize = 1 << 20;

struct SegInner {
    file: File,
    pending: Vec<u8>,
    flushed: u64,
}

impl SegInner {
    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.file.seek(SeekFrom::Start(self.flushed))?;
        self.file.write_all(&self.pending)?;
        self.flushed += self.pending.len() as u64;
        self.pending.clear();
        Ok(())
    }
}

/// One file of packet data; deleted when the last packet that lives in it is forgotten (a save in
/// progress keeps it alive).
struct Segment {
    path: PathBuf,
    inner: Mutex<SegInner>,
}

impl Segment {
    fn create(path: PathBuf) -> io::Result<Arc<Self>> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        Ok(Arc::new(Self {
            path,
            inner: Mutex::new(SegInner {
                file,
                pending: Vec::new(),
                flushed: 0,
            }),
        }))
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, SegInner>> {
        self.inner
            .lock()
            .map_err(|_| io::Error::other("replay segment lock poisoned"))
    }

    /// Appends `data`; gives its offset.
    fn append(&self, data: &[u8]) -> io::Result<u64> {
        let mut inner = self.lock()?;
        let offset = inner.flushed + inner.pending.len() as u64;
        inner.pending.extend_from_slice(data);
        if inner.pending.len() >= WRITE_BLOCK {
            inner.flush()?;
        }
        Ok(offset)
    }

    fn len(&self) -> u64 {
        self.lock()
            .map_or(0, |i| i.flushed + i.pending.len() as u64)
    }

    fn read(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut inner = self.lock()?;
        if offset + len as u64 > inner.flushed {
            inner.flush()?;
        }
        inner.file.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; len];
        inner.file.read_exact(&mut data)?;
        Ok(data)
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A packet kept in a segment file: its data is on disk, its timestamps here.
pub struct DiskPacket {
    segment: Arc<Segment>,
    offset: u64,
    len: usize,
    pts: Option<i64>,
    dts: Option<i64>,
    duration: i64,
    flags: ffmpeg_next::codec::packet::Flags,
}

/// A packet of the ring, in RAM or on disk.
#[derive(Clone)]
pub enum Held {
    Ram(Arc<Packet>),
    Disk(Arc<DiskPacket>),
}

impl Held {
    pub fn size(&self) -> usize {
        match self {
            Self::Ram(p) => p.size(),
            Self::Disk(d) => d.len,
        }
    }

    pub fn pts(&self) -> Option<i64> {
        match self {
            Self::Ram(p) => p.pts(),
            Self::Disk(d) => d.pts,
        }
    }

    pub fn is_key(&self) -> bool {
        match self {
            Self::Ram(p) => p.is_key(),
            Self::Disk(d) => d.flags.contains(ffmpeg_next::codec::packet::Flags::KEY),
        }
    }

    /// A packet to write: shares the data in RAM, reads it back from the file on disk.
    pub fn load(&self) -> io::Result<Packet> {
        match self {
            Self::Ram(p) => Ok(share(p)),
            Self::Disk(d) => {
                let data = d.segment.read(d.offset, d.len)?;
                let mut packet = Packet::copy(&data);
                packet.set_pts(d.pts);
                packet.set_dts(d.dts);
                packet.set_duration(d.duration);
                packet.set_flags(d.flags);
                Ok(packet)
            }
        }
    }
}

/// The folder and the current segment of a ring that keeps its packets on disk.
struct DiskStore {
    dir: PathBuf,
    current: Option<Arc<Segment>>,
    next_id: u32,
}

impl DiskStore {
    fn new(base: &Path) -> io::Result<Self> {
        // Folders left by a crash are removed once they are a day old.
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let old = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 24 * 3600);
                if old {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        static RINGS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = RINGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = base.join(format!("ring-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            current: None,
            next_id: 0,
        })
    }

    /// Writes `packet`; a key frame may start a new segment.
    fn store(&mut self, packet: &Packet, may_roll: bool) -> io::Result<Held> {
        let roll = self
            .current
            .as_ref()
            .is_none_or(|s| may_roll && s.len() >= SEGMENT_BYTES);
        if roll {
            let path = self.dir.join(format!("seg-{}.bin", self.next_id));
            self.next_id += 1;
            self.current = Some(Segment::create(path)?);
        }
        let segment = self
            .current
            .as_ref()
            .ok_or_else(|| io::Error::other("no segment"))?;
        let data = packet.data().unwrap_or_default();
        let offset = segment.append(data)?;
        Ok(Held::Disk(Arc::new(DiskPacket {
            segment: Arc::clone(segment),
            offset,
            len: data.len(),
            pts: packet.pts(),
            dts: packet.dts(),
            duration: packet.duration(),
            flags: packet.flags(),
        })))
    }
}

impl Drop for DiskStore {
    fn drop(&mut self) {
        self.current = None;
        // Empty once the segments are gone; a save still running keeps its files and the folder
        // (the stale-folder cleanup of a later run removes it).
        let _ = std::fs::remove_dir(&self.dir);
    }
}

struct Entry {
    /// Media time of the packet, in samples.
    t: i64,
    packet: Held,
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
    pub video: Vec<(i64, Held)>,
    /// One list per audio track, packets from `start` on.
    pub audio: Vec<Vec<(i64, Held)>>,
}

pub struct Ring {
    /// How much to keep, in samples.
    keep: i64,
    gops: VecDeque<Gop>,
    audio: Vec<VecDeque<Entry>>,
    bytes: usize,
    newest: i64,
    store: Option<DiskStore>,
}

impl Ring {
    pub fn new(seconds: u32, tracks: usize) -> Self {
        Self::with_storage(seconds, tracks, &Storage::Ram)
    }

    /// A ring on disk falls back to RAM when the folder cannot be used.
    pub fn with_storage(seconds: u32, tracks: usize, storage: &Storage) -> Self {
        let store = match storage {
            Storage::Ram => None,
            Storage::Disk(base) => DiskStore::new(base)
                .inspect_err(|e| tracing::warn!("replay on disk unavailable, using RAM: {e}"))
                .ok(),
        };
        Self {
            keep: i64::from(seconds) * i64::from(SAMPLE_RATE),
            gops: VecDeque::new(),
            audio: (0..tracks).map(|_| VecDeque::new()).collect(),
            bytes: 0,
            newest: i64::MIN,
            store,
        }
    }

    /// Where a packet goes: the disk when there is one, else (or when writing fails) RAM.
    fn keep(&mut self, packet: &Packet, may_roll: bool) -> Held {
        if let Some(store) = &mut self.store {
            match store.store(packet, may_roll) {
                Ok(held) => return held,
                Err(e) => {
                    tracing::warn!("replay on disk failed, using RAM from now on: {e}");
                    self.store = None;
                }
            }
        }
        Held::Ram(Arc::new(share(packet)))
    }

    /// Compressed bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn push_video(&mut self, t: i64, packet: &Packet) {
        let size = packet.size();
        if packet.is_key() || self.gops.is_empty() {
            // Packets before the first key frame cannot be decoded on their own: not kept.
            if !packet.is_key() {
                return;
            }
        }
        let entry = Entry {
            t,
            packet: self.keep(packet, packet.is_key()),
        };
        if packet.is_key() || self.gops.is_empty() {
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
        if track >= self.audio.len() {
            return;
        }
        let held = self.keep(packet, false);
        self.bytes += held.size();
        self.audio[track].push_back(Entry { t, packet: held });
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
            .flat_map(|g| g.packets.iter().map(|e| (e.t, e.packet.clone())))
            .collect();
        let audio = self
            .audio
            .iter()
            .map(|q| {
                q.iter()
                    .filter(|e| e.t >= start)
                    .map(|e| (e.t, e.packet.clone()))
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
    use std::path::PathBuf;

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

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vixeeny-ring-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn packets_kept_on_disk_come_back_identical() {
        let base = scratch("identical");
        let mut ring = Ring::with_storage(10, 1, &Storage::Disk(base.clone()));
        for i in 0..90_i64 {
            let mut p = packet(500 + i as usize, i % 30 == 0);
            p.set_pts(Some(i * 3));
            p.set_dts(Some(i * 3 - 1));
            p.set_duration(3);
            ring.push_video(i * S / 30, &p);
            ring.push_audio(0, i * S / 30, &packet(40, true));
        }
        let snap = ring.snapshot(10).unwrap();
        let (_, first) = &snap.video[0];
        assert!(first.is_key());
        assert_eq!(first.pts(), Some(0));
        let loaded = first.load().unwrap();
        assert_eq!(loaded.size(), 500);
        assert_eq!(loaded.dts(), Some(-1));
        assert_eq!(loaded.duration(), 3);
        assert!(loaded.is_key());
        let (_, last) = snap.video.last().unwrap();
        assert_eq!(last.load().unwrap().size(), 500 + 89);
        assert_eq!(snap.audio[0].len(), 90);
        assert!(
            snap.audio[0]
                .iter()
                .all(|(_, p)| p.load().unwrap().size() == 40)
        );
        drop(snap);
        drop(ring);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn old_segments_are_deleted_and_the_disk_use_stays_bounded() {
        let base = scratch("bounded");
        let mut ring = Ring::with_storage(5, 0, &Storage::Disk(base.clone()));
        // 20 MB per second for a minute: well past one segment, far more than the 5 s kept.
        fill(&mut ring, 60, 160_000);
        let files = |base: &Path| -> usize {
            std::fs::read_dir(base)
                .unwrap()
                .flatten()
                .flat_map(|ring| std::fs::read_dir(ring.path()).unwrap().flatten())
                .count()
        };
        let used = files(&base);
        assert!((1..=5).contains(&used), "{used} segment files");
        drop(ring);
        assert_eq!(files(&base), 0);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_save_in_progress_keeps_its_segments_alive() {
        let base = scratch("alive");
        let mut ring = Ring::with_storage(5, 0, &Storage::Disk(base.clone()));
        fill(&mut ring, 20, 8_000);
        let snap = ring.snapshot(5).unwrap();
        drop(ring);
        assert!(snap.video.iter().all(|(_, p)| p.load().is_ok()));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn an_unusable_folder_falls_back_to_ram() {
        let base = scratch("unusable");
        std::fs::create_dir_all(base.parent().unwrap()).unwrap();
        std::fs::write(&base, "a file where the folder should be").unwrap();
        let mut ring = Ring::with_storage(5, 0, &Storage::Disk(base.clone()));
        fill(&mut ring, 10, 800);
        assert!(ring.snapshot(5).unwrap().video[0].1.load().is_ok());
        let _ = std::fs::remove_file(base);
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
