// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording pipeline (plan 4.4, 5.9): frames with a monotonic timestamp go through the
//! constant-frame-rate clock ([`crate::clock`]), colour conversion and scaling, an encoder opened
//! from the registry, and a muxer (MKV, hybrid or fragmented MP4, WebM) that can split the output
//! on key frames. Everything runs on a dedicated worker thread behind a bounded queue, so a slow
//! encoder drops input frames (counted) instead of stalling the capture.
//!
//! Frames come either as BGRA in RAM (the CPU path: converted here) or already converted on the
//! GPU ([`crate::gpu`]): handed to NVENC / AMF / QSV as they are, downloaded for a software
//! encoder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling;
use ffmpeg_next::{Dictionary, Packet, Rational, codec, color, encoder, ffi, format, frame};

use crate::audio::{AudioEncoder, AudioTrackConfig, SAMPLE_RATE};
use crate::clock::{Cfr, Emit, Fps};
use crate::gpu::{GpuPipeline, HwFrame};
use crate::registry::{Chroma, Encoder};
use crate::replay::{Ring, Snapshot, Storage};

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("{0}")]
    Config(String),
    #[error("ffmpeg: {0}")]
    Ffmpeg(String),
    #[error("file: {0}")]
    Io(String),
    #[error("the recording thread stopped unexpectedly")]
    Worker,
}

impl From<ffmpeg_next::Error> for RecordError {
    fn from(e: ffmpeg_next::Error) -> Self {
        Self::Ffmpeg(e.to_string())
    }
}

/// The file format of a recording (profile `container` setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputContainer {
    Mkv,
    /// Fragmented while recording (readable after a crash), classic with `moov` first once done.
    Mp4Hybrid,
    Mp4Fragmented,
    WebM,
}

impl OutputContainer {
    pub fn from_setting(name: &str) -> Option<Self> {
        match name {
            "mkv" => Some(Self::Mkv),
            "mp4" | "mp4_hybrid" => Some(Self::Mp4Hybrid),
            "fmp4" | "mp4_fragmented" => Some(Self::Mp4Fragmented),
            "webm" => Some(Self::WebM),
            _ => None,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Mkv => "mkv",
            Self::Mp4Hybrid | Self::Mp4Fragmented => "mp4",
            Self::WebM => "webm",
        }
    }

    const fn muxer(self) -> &'static str {
        match self {
            Self::Mkv => "matroska",
            Self::Mp4Hybrid | Self::Mp4Fragmented => "mp4",
            Self::WebM => "webm",
        }
    }

    fn header_options(self) -> Dictionary<'static> {
        let mut d = Dictionary::new();
        // Every packet reaches the disk at once: what a crash leaves behind is what was written.
        d.set("flush_packets", "1");
        if matches!(self, Self::Mp4Hybrid | Self::Mp4Fragmented) {
            d.set("movflags", "frag_keyframe+empty_moov+default_base_moof");
        }
        d
    }
}

/// When to start a new file (always on a key frame, without losing a frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Split {
    Off,
    Bytes(u64),
    Duration(Duration),
}

#[derive(Debug, Clone)]
pub struct RecordConfig {
    pub encoder: Encoder,
    /// FFmpeg options of the encoder: the preset's, plus the user's.
    pub options: Vec<(String, String)>,
    pub container: OutputContainer,
    pub output_size: (u32, u32),
    /// Keep only the middle of the picture with this ratio (`None`: all of it). GPU frames
    /// come cropped already.
    pub crop: Option<(u32, u32)>,
    pub fps: Fps,
    pub depth: u8,
    pub chroma: Chroma,
    /// Tag the stream BT.2020 / PQ (HDR10). The pixels must already be PQ-encoded.
    pub hdr: bool,
    pub split: Split,
    /// Key frame interval, seconds (also the granularity of crash recovery and of splitting).
    pub keyframe_seconds: f64,
    /// Frames the queue holds before input frames are dropped.
    pub queue: usize,
    /// Variable frame rate: every captured frame is kept at its own time, nothing is repeated
    /// or dropped (the file is smaller when the screen is mostly still). Matroska and WebM only.
    pub vfr: bool,
    /// Audio tracks, in file order; fed with `push_audio` (empty = no audio).
    pub audio: Vec<AudioTrackConfig>,
    /// Hardware encoders that read D3D11 frames (Windows): frames come from `push_hw`.
    pub gpu: Option<Arc<GpuPipeline>>,
    /// Keep the last seconds of encoded packets in RAM for [`Recorder::save_replay`] (plan 5.11).
    pub replay_seconds: Option<u32>,
    /// Where the replay keeps its packets.
    pub replay_storage: Storage,
    /// Write files. A replay-only session has none: the namer is never called.
    pub files: bool,
}

/// How the pixels of a [`VideoFrame`] are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    /// 8-bit BGRA, sRGB (an SDR monitor).
    Bgra8,
    /// RGBA half floats, scRGB (an HDR monitor): converted to BT.2020 / PQ by the recorder.
    ScRgbHalf,
}

/// A captured frame in RAM.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: FrameFormat,
    pub data: Vec<u8>,
}

/// Live counters, readable while recording (the "frames lost" figure of CA-REC-1).
#[derive(Debug, Default)]
pub struct Stats {
    pub pushed: AtomicU64,
    /// Input frames refused because the encoder could not keep up.
    pub queue_dropped: AtomicU64,
    pub encoded: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    pub files: Vec<PathBuf>,
    /// Frames in the output.
    pub frames: u64,
    /// Output frames that repeat the previous one (the source was late).
    pub repeated: u64,
    /// Input frames the clock discarded (they arrived before their slot).
    pub dropped_by_clock: u64,
    pub dropped_by_queue: u64,
}

/// What a frame command carries.
enum Input {
    Cpu(VideoFrame),
    Hw(HwFrame),
}

/// The last converted frame, repeated when the source is late.
enum Last {
    Sw(frame::Video),
    Hw(HwFrame),
}

enum Cmd {
    Frame(i64, Box<Input>),
    Audio(usize, Vec<f32>),
    Tick(i64),
    Pause(i64),
    Resume(i64),
    Stop(i64),
    Save(PathBuf, SyncSender<Result<ReplaySave, RecordError>>),
}

/// A replay being written (on a thread of its own, so the recording is not held up).
pub struct ReplaySave {
    pub path: PathBuf,
    /// Seconds of media in the file.
    pub seconds: f64,
    join: JoinHandle<Result<(), RecordError>>,
}

impl ReplaySave {
    /// Waits for the file to be complete.
    pub fn wait(self) -> Result<PathBuf, RecordError> {
        self.join.join().map_err(|_| RecordError::Worker)??;
        Ok(self.path)
    }
}

pub struct Recorder {
    tx: SyncSender<Cmd>,
    join: JoinHandle<Result<Summary, RecordError>>,
    stats: Arc<Stats>,
}

type Namer = Box<dyn FnMut(u32) -> PathBuf + Send>;

impl Recorder {
    /// Opens the encoder and the first file, then records until [`Recorder::stop`]. `namer`
    /// gives the path of each part (0, 1, …).
    pub fn start(config: RecordConfig, namer: Namer) -> Result<Self, RecordError> {
        let stats = Arc::new(Stats::default());
        let (tx, rx) = sync_channel(config.queue.max(1));
        let (ready_tx, ready_rx) = sync_channel(1);
        let worker_stats = stats.clone();
        let join = std::thread::Builder::new()
            .name("vixeeny-record".into())
            .spawn(move || {
                let _ = ffmpeg_next::init();
                ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);
                let worker = match Worker::open(config, namer, worker_stats) {
                    Ok(w) => {
                        let _ = ready_tx.send(Ok(()));
                        w
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return Err(e);
                    }
                };
                worker.run(&rx)
            })
            .map_err(|e| RecordError::Io(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx, join, stats }),
            Ok(Err(message)) => {
                let _ = join.join();
                Err(RecordError::Config(message))
            }
            Err(_) => Err(RecordError::Worker),
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Queues a frame captured at `t` (nanoseconds, monotonic). `false` = dropped, the encoder
    /// is behind.
    pub fn push_frame(&self, t: i64, frame: VideoFrame) -> bool {
        self.stats.pushed.fetch_add(1, Ordering::Relaxed);
        self.queue(t, Input::Cpu(frame))
    }

    /// Queues a frame already converted on the GPU (see [`GpuPipeline::convert`]).
    pub fn push_hw(&self, t: i64, frame: HwFrame) -> bool {
        self.stats.pushed.fetch_add(1, Ordering::Relaxed);
        self.queue(t, Input::Hw(frame))
    }

    fn queue(&self, t: i64, input: Input) -> bool {
        match self.tx.try_send(Cmd::Frame(t, Box::new(input))) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.stats.queue_dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Appends samples (48 kHz, interleaved f32, in the track's channel count) to audio track
    /// `track`. The track is one gapless stream starting at the recording's time zero (the origin
    /// of the video timestamps): `vixeeny_audio::Mixer` produces exactly that. Blocks when the encoder is behind, so no
    /// audio is ever dropped (a dropped block would shift the track).
    pub fn push_audio(&self, track: usize, samples: Vec<f32>) {
        let _ = self.tx.send(Cmd::Audio(track, samples));
    }

    /// Call regularly (a few times per second) so a static screen keeps its frame rate.
    pub fn tick(&self, t: i64) {
        let _ = self.tx.try_send(Cmd::Tick(t));
    }

    pub fn pause(&self, t: i64) {
        let _ = self.tx.send(Cmd::Pause(t));
    }

    pub fn resume(&self, t: i64) {
        let _ = self.tx.send(Cmd::Resume(t));
    }

    /// Writes the replay buffer's last seconds to `path` without re-encoding. Returns once the
    /// packets are picked (the buffer keeps running); the file is complete after
    /// [`ReplaySave::wait`].
    pub fn save_replay(&self, path: PathBuf) -> Result<ReplaySave, RecordError> {
        let (reply, answer) = sync_channel(1);
        self.tx
            .send(Cmd::Save(path, reply))
            .map_err(|_| RecordError::Worker)?;
        answer.recv().map_err(|_| RecordError::Worker)?
    }

    /// Finishes the files and returns what was written.
    pub fn stop(self, t: i64) -> Result<Summary, RecordError> {
        let _ = self.tx.send(Cmd::Stop(t));
        drop(self.tx);
        self.join.join().map_err(|_| RecordError::Worker)?
    }
}

/// Time stamping of a variable-rate recording: microseconds from the first frame, pauses cut
/// out, strictly increasing.
#[derive(Debug, Default)]
struct Vfr {
    origin: Option<i64>,
    paused_at: Option<i64>,
    paused_total: i64,
    last: Option<i64>,
}

impl Vfr {
    /// The stamp of a frame captured at `t` (ns); `None` while paused.
    fn stamp(&mut self, t: i64) -> Option<u64> {
        if self.paused_at.is_some() {
            return None;
        }
        let origin = *self.origin.get_or_insert(t);
        let mut pts = (t - origin - self.paused_total) / 1000;
        if let Some(last) = self.last {
            pts = pts.max(last + 1);
        }
        self.last = Some(pts);
        Some(pts.max(0) as u64)
    }

    fn pause(&mut self, t: i64) {
        self.paused_at.get_or_insert(t);
    }

    fn resume(&mut self, t: i64) {
        if let Some(at) = self.paused_at.take() {
            self.paused_total += (t - at).max(0);
        }
    }
}

/// The encoder's pixel format for a depth and a chroma, from the registry entry.
fn pixel_format(encoder: &Encoder, depth: u8, chroma: Chroma) -> Result<Pixel, RecordError> {
    let spec = encoder
        .pixel_formats
        .iter()
        .find(|p| p.depth == depth && p.chroma == chroma)
        .ok_or_else(|| {
            RecordError::Config(format!(
                "{} does not take {depth}-bit {} video",
                encoder.display_name,
                chroma.name()
            ))
        })?;
    <Pixel as std::str::FromStr>::from_str(&spec.ffmpeg)
        .map_err(|_| RecordError::Config(format!("unknown pixel format {}", spec.ffmpeg)))
}

fn dictionary(options: &[(String, String)]) -> Dictionary<'static> {
    let mut d = Dictionary::new();
    for (k, v) in options {
        d.set(k, v);
    }
    d
}

struct Output {
    octx: format::context::Output,
    path: PathBuf,
    stream_tb: Rational,
    /// pts of the first (key) packet of the part, in encoder time base: subtracted so each file
    /// starts at zero (the dts stay negative by the encoder delay, as in the first file).
    offset: Option<i64>,
    first_pts: i64,
    packets: u64,
    /// Compressed bytes of the part, for the size limit.
    bytes: u64,
    /// Time bases of the audio streams (stream `1 + i`).
    audio_tb: Vec<Rational>,
    /// Samples subtracted from the audio pts so the part starts at zero, like the video.
    audio_cut: i64,
}

fn samples_of(tb: Rational, pts: i64) -> i64 {
    let num = i128::from(tb.0) * i128::from(SAMPLE_RATE);
    (i128::from(pts) * num / i128::from(tb.1.max(1))) as i64
}

/// An audio track on its way to the file.
struct AudioTrack {
    enc: AudioEncoder,
    codec: ffmpeg_next::Codec,
    title: String,
    /// Samples that arrived before the first video frame fixed the time zero.
    pending: Vec<f32>,
    /// Frames still to drop from the start (the video starts a little after the clock origin).
    to_skip: usize,
    /// Encoded packets waiting for the video to catch up (a split must cut both by time).
    held: std::collections::VecDeque<Packet>,
}

struct Worker {
    cfg: RecordConfig,
    namer: Namer,
    stats: Arc<Stats>,
    cfr: Cfr,
    vfr: Vfr,
    encoder: encoder::Video,
    codec: ffmpeg_next::Codec,
    pix: Pixel,
    enc_tb: Rational,
    scaler: Option<(scaling::Context, (u32, u32, FrameFormat))>,
    /// Downloaded GPU frames (NV12 / P010) to the encoder's planar format: a reshuffle, no
    /// colour conversion.
    unpack: Option<(scaling::Context, Pixel)>,
    last: Option<Last>,
    out: Option<Output>,
    audio: Vec<AudioTrack>,
    /// Time zero of the media: the first video frame; `None` until it arrives.
    anchored: bool,
    /// Latest video pts written, in audio samples.
    horizon: i64,
    part: u32,
    files: Vec<PathBuf>,
    ring: Option<Ring>,
}

impl Worker {
    fn open(cfg: RecordConfig, mut namer: Namer, stats: Arc<Stats>) -> Result<Self, RecordError> {
        let pix = pixel_format(&cfg.encoder, cfg.depth, cfg.chroma)?;
        let codec = encoder::find_by_name(&cfg.encoder.ffmpeg_encoder).ok_or_else(|| {
            RecordError::Config(format!("{} is not built in", cfg.encoder.ffmpeg_encoder))
        })?;
        let (w, h) = cfg.output_size;
        if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
            return Err(RecordError::Config(format!(
                "bad output size {w}×{h} (must be even)"
            )));
        }
        let first = if cfg.files {
            let path0 = namer(0);
            if let Some(dir) = path0.parent() {
                std::fs::create_dir_all(dir).map_err(|e| RecordError::Io(e.to_string()))?;
            }
            Some((format::output_as(&path0, cfg.container.muxer())?, path0))
        } else {
            None
        };
        // Every container we write (Matroska, MP4, WebM) wants the codec headers out of band.
        let global = first
            .as_ref()
            .is_none_or(|(octx, _)| octx.format().flags().contains(format::Flags::GLOBAL_HEADER));

        let mut ctx = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        ctx.set_width(w);
        ctx.set_height(h);
        ctx.set_format(pix);
        if let Some(gpu) = &cfg.gpu {
            crate::gpu::attach(&mut ctx, gpu)?;
        }
        if cfg.vfr && !matches!(cfg.container, OutputContainer::Mkv | OutputContainer::WebM) {
            return Err(RecordError::Config(
                "variable frame rate needs Matroska or WebM".into(),
            ));
        }
        // VFR stamps frames in microseconds; CFR counts frames.
        let enc_tb = if cfg.vfr {
            Rational(1, 1_000_000)
        } else {
            Rational(cfg.fps.den as i32, cfg.fps.num as i32)
        };
        ctx.set_time_base(enc_tb);
        ctx.set_frame_rate(Some(Rational(cfg.fps.num as i32, cfg.fps.den as i32)));
        let gop = (f64::from(cfg.fps.num) / f64::from(cfg.fps.den) * cfg.keyframe_seconds).round();
        ctx.set_gop(gop.max(1.0) as u32);
        // Closed GOPs: a key frame never has leading frames that need the previous part.
        let mut flags = codec::Flags::CLOSED_GOP;
        ctx.set_color_range(color::Range::MPEG);
        if cfg.hdr {
            ctx.set_colorspace(color::Space::BT2020NCL);
            ctx.set_color_primaries(color::Primaries::BT2020);
            ctx.set_color_transfer_characteristic(color::TransferCharacteristic::SMPTE2084);
        } else {
            ctx.set_colorspace(color::Space::BT709);
            ctx.set_color_primaries(color::Primaries::BT709);
            ctx.set_color_transfer_characteristic(color::TransferCharacteristic::BT709);
        }
        if global {
            flags |= codec::Flags::GLOBAL_HEADER;
        }
        ctx.set_flags(flags);
        let mut options = dictionary(&cfg.options);
        // FFmpeg's default is one thread: libvpx then encodes on a single core and x265 one
        // frame at a time. Zero lets each encoder use the whole CPU (x264 does by itself).
        if options.get("threads").is_none() {
            options.set("threads", "0");
        }
        let encoder = ctx.open_with(options)?;

        let mut worker = Self {
            cfr: Cfr::new(cfg.fps),
            vfr: Vfr::default(),
            cfg,
            namer,
            stats,
            encoder,
            codec,
            pix,
            enc_tb,
            scaler: None,
            unpack: None,
            last: None,
            out: None,
            audio: Vec::new(),
            anchored: false,
            horizon: i64::MIN,
            part: 0,
            files: Vec::new(),
            ring: None,
        };
        worker.ring = worker.cfg.replay_seconds.map(|seconds| {
            Ring::with_storage(seconds, worker.cfg.audio.len(), &worker.cfg.replay_storage)
        });
        for track in &worker.cfg.audio {
            let enc = AudioEncoder::open(track, global)?;
            let codec = enc.codec;
            worker.audio.push(AudioTrack {
                enc,
                codec,
                title: track.title.clone(),
                pending: Vec::new(),
                to_skip: 0,
                held: std::collections::VecDeque::new(),
            });
        }
        if let Some((octx, path0)) = first {
            worker.begin_part(octx, path0)?;
        }
        Ok(worker)
    }

    /// The first video frame fixes the media's time zero. Audio starts at the clock origin, so
    /// the audio that came before the frame is cut: both streams then agree on what "0" is.
    fn anchor(&mut self, t: i64) -> Result<(), RecordError> {
        if self.anchored {
            return Ok(());
        }
        self.anchored = true;
        let skip = (i128::from(t.max(0)) * i128::from(SAMPLE_RATE) + 500_000_000) / 1_000_000_000;
        for i in 0..self.audio.len() {
            self.audio[i].to_skip = skip as usize;
            let pending = std::mem::take(&mut self.audio[i].pending);
            self.audio_in(i, pending)?;
        }
        Ok(())
    }

    fn audio_in(&mut self, track: usize, mut samples: Vec<f32>) -> Result<(), RecordError> {
        let Some(t) = self.audio.get_mut(track) else {
            return Ok(());
        };
        if !self.anchored {
            t.pending.extend_from_slice(&samples);
            return Ok(());
        }
        let channels = t.enc.channels();
        let skip = t.to_skip.min(samples.len() / channels);
        t.to_skip -= skip;
        samples.drain(..skip * channels);
        if samples.is_empty() {
            return Ok(());
        }
        let packets = t.enc.push(&samples)?;
        t.held.extend(packets);
        self.write_ready_audio()
    }

    /// Video pts (encoder time base) as audio samples.
    fn pts_to_samples(&self, pts: i64) -> i64 {
        samples_of(self.enc_tb, pts)
    }

    /// Writes the held audio packets that the video has caught up with.
    fn write_ready_audio(&mut self) -> Result<(), RecordError> {
        self.write_audio_before(self.horizon)
    }

    /// Writes the held audio packets that start at or before `limit` samples (media time).
    fn write_audio_before(&mut self, limit: i64) -> Result<(), RecordError> {
        for i in 0..self.audio.len() {
            while self.audio[i]
                .held
                .front()
                .is_some_and(|p| p.pts().unwrap_or(0) <= limit)
            {
                if let Some(mut packet) = self.audio[i].held.pop_front() {
                    self.write_audio(i, &mut packet)?;
                }
            }
        }
        Ok(())
    }

    fn write_audio(&mut self, track: usize, packet: &mut Packet) -> Result<(), RecordError> {
        if let (Some(ring), Some(pts)) = (&mut self.ring, packet.pts()) {
            ring.push_audio(track, pts, packet);
        }
        let Some(out) = &mut self.out else {
            return Ok(());
        };
        let Some(pts) = packet.pts().map(|p| p - out.audio_cut) else {
            return Ok(());
        };
        if pts < 0 {
            return Ok(()); // belongs to the previous part
        }
        packet.set_pts(Some(pts));
        packet.set_dts(packet.dts().map(|d| d - out.audio_cut));
        let tb = out.audio_tb.get(track).copied().unwrap_or(self.enc_tb);
        packet.rescale_ts(Rational(1, SAMPLE_RATE as i32), tb);
        packet.set_stream(1 + track);
        packet.write_interleaved(&mut out.octx)?;
        Ok(())
    }

    /// Encodes the tail of every track and writes everything still held.
    fn finish_audio(&mut self) -> Result<(), RecordError> {
        if !self.anchored {
            self.anchor(0)?;
        }
        for i in 0..self.audio.len() {
            let tail = self.audio[i].enc.finish()?;
            self.audio[i].held.extend(tail);
        }
        self.write_audio_before(i64::MAX)
    }

    fn begin_part(
        &mut self,
        octx: format::context::Output,
        path: PathBuf,
    ) -> Result<(), RecordError> {
        self.files.push(path.clone());
        self.out = Some(self.new_output(octx, path)?);
        Ok(())
    }

    /// The streams of a file (video, then the audio tracks) and its header.
    fn new_output(
        &self,
        mut octx: format::context::Output,
        path: PathBuf,
    ) -> Result<Output, RecordError> {
        let mut stream = octx.add_stream(self.codec)?;
        stream.set_parameters(&self.encoder);
        for track in &self.audio {
            let mut stream = octx.add_stream(track.codec)?;
            stream.set_parameters(&track.enc.encoder);
            let mut meta = Dictionary::new();
            meta.set("title", &track.title);
            // MP4 keeps a stream's name in the handler, not in a title tag.
            meta.set("handler_name", &track.title);
            stream.set_metadata(meta);
        }
        octx.write_header_with(self.cfg.container.header_options())?;
        let stream_tb = octx.stream(0).map_or(self.enc_tb, |s| s.time_base());
        let audio_tb = (0..self.audio.len())
            .map(|i| {
                octx.stream(1 + i)
                    .map_or(Rational(1, SAMPLE_RATE as i32), |s| s.time_base())
            })
            .collect();
        Ok(Output {
            octx,
            path,
            stream_tb,
            offset: None,
            first_pts: 0,
            packets: 0,
            bytes: 0,
            audio_tb,
            audio_cut: 0,
        })
    }

    /// Picks the packets of a save and hands them to a thread that writes the file.
    fn save_replay(&mut self, path: PathBuf) -> Result<ReplaySave, RecordError> {
        let seconds = self
            .cfg
            .replay_seconds
            .ok_or_else(|| RecordError::Config("this recording has no replay buffer".into()))?;
        let snapshot = self
            .ring
            .as_ref()
            .and_then(|r| r.snapshot(seconds))
            .ok_or_else(|| RecordError::Config("the replay buffer is still empty".into()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| RecordError::Io(e.to_string()))?;
        }
        let octx = format::output_as(&path, self.cfg.container.muxer())?;
        let mut out = self.new_output(octx, path.clone())?;
        let enc_tb = self.enc_tb;
        let container = self.cfg.container;
        let last = snapshot.video.last().map_or(snapshot.start, |(t, _)| *t);
        let span = (last - snapshot.start) as f64 / f64::from(SAMPLE_RATE);
        let join = std::thread::Builder::new()
            .name("vixeeny-replay-save".into())
            .spawn(move || {
                write_snapshot(&mut out, &snapshot, enc_tb)?;
                let path = out.path.clone();
                out.octx.write_trailer()?;
                drop(out);
                if container == OutputContainer::Mp4Hybrid {
                    faststart(&path)?;
                }
                Ok(())
            })
            .map_err(|e| RecordError::Io(e.to_string()))?;
        Ok(ReplaySave {
            path,
            seconds: span,
            join,
        })
    }

    fn run(mut self, rx: &Receiver<Cmd>) -> Result<Summary, RecordError> {
        let mut stop_at = None;
        while let Ok(cmd) = rx.recv() {
            let emits = match cmd {
                Cmd::Audio(track, samples) => {
                    self.audio_in(track, samples)?;
                    continue;
                }
                Cmd::Frame(t, frame) if self.cfg.vfr => {
                    self.anchor(t)?;
                    if let Some(pts) = self.vfr.stamp(t) {
                        self.last = Some(self.prepare(*frame)?);
                        self.send(pts)?;
                    }
                    continue;
                }
                Cmd::Tick(_) if self.cfg.vfr => continue,
                Cmd::Pause(t) if self.cfg.vfr => {
                    self.vfr.pause(t);
                    continue;
                }
                Cmd::Resume(t) if self.cfg.vfr => {
                    self.vfr.resume(t);
                    continue;
                }
                Cmd::Frame(t, frame) => {
                    self.anchor(t)?;
                    let emits = self.cfr.on_frame(t);
                    self.emit(&emits, Some(*frame))?;
                    continue;
                }
                Cmd::Tick(t) => self.cfr.on_tick(t),
                Cmd::Pause(t) => self.cfr.pause(t),
                Cmd::Resume(t) => {
                    self.cfr.resume(t);
                    continue;
                }
                Cmd::Save(path, reply) => {
                    let _ = reply.send(self.save_replay(path));
                    continue;
                }
                Cmd::Stop(t) => {
                    stop_at = Some(t);
                    break;
                }
            };
            self.emit(&emits, None)?;
        }
        if let Some(t) = stop_at.filter(|_| !self.cfg.vfr) {
            let emits = self.cfr.finish(t);
            self.emit(&emits, None)?;
        }
        self.finish()
    }

    /// Sends the planned frames to the encoder. `new` is the frame for the `Emit::New` slot.
    fn emit(&mut self, emits: &[Emit], mut new: Option<Input>) -> Result<(), RecordError> {
        for e in emits {
            match *e {
                Emit::New(i) => {
                    if let Some(input) = new.take() {
                        self.last = Some(self.prepare(input)?);
                        self.send(i)?;
                    }
                }
                Emit::Repeat(i) => self.send(i)?,
            }
        }
        Ok(())
    }

    /// Software frames are converted and scaled here; GPU frames already are.
    fn prepare(&mut self, input: Input) -> Result<Last, RecordError> {
        let feed = self.cfg.gpu.as_ref().map(|g| g.feed());
        Ok(match (input, feed) {
            (Input::Cpu(frame), _) => Last::Sw(self.convert(&frame)?),
            (Input::Hw(frame), Some(crate::gpu::Feed::Qsv)) => {
                let gpu = self.cfg.gpu.as_ref().ok_or(RecordError::Worker)?;
                Last::Hw(gpu.to_qsv(&frame)?)
            }
            (Input::Hw(frame), Some(crate::gpu::Feed::Download)) => {
                Last::Sw(self.download(&frame)?)
            }
            (Input::Hw(frame), _) => Last::Hw(frame),
        })
    }

    /// A GPU frame in system memory, in the encoder's pixel format.
    fn download(&mut self, frame: &HwFrame) -> Result<frame::Video, RecordError> {
        let gpu = self.cfg.gpu.as_ref().ok_or(RecordError::Worker)?;
        let got = gpu.download(frame)?;
        if got.format() == self.pix {
            return Ok(got);
        }
        if self
            .unpack
            .as_ref()
            .is_none_or(|(_, from)| *from != got.format())
        {
            let ctx = scaling::Context::get(
                got.format(),
                got.width(),
                got.height(),
                self.pix,
                got.width(),
                got.height(),
                scaling::Flags::POINT,
            )?;
            self.unpack = Some((ctx, got.format()));
        }
        let mut out = frame::Video::new(self.pix, got.width(), got.height());
        if let Some((ctx, _)) = &mut self.unpack {
            ctx.run(&got, &mut out)?;
        }
        Ok(out)
    }

    fn convert(&mut self, src: &VideoFrame) -> Result<frame::Video, RecordError> {
        let (ow, oh) = self.cfg.output_size;
        let (left, top, width, height) =
            crate::validate::center_crop((src.width, src.height), self.cfg.crop);
        let key = (width, height, src.format);
        // Half-float HDR frames become 16-bit PQ RGB first; swscale does the rest.
        let hdr16;
        let (input_pix, bytes, stride, bpp) = match src.format {
            FrameFormat::Bgra8 => (Pixel::BGRA, &src.data[..], src.stride, 4),
            FrameFormat::ScRgbHalf => {
                hdr16 = crate::hdr::scrgb_half_to_pq(&src.data, src.width, src.height, src.stride)
                    .ok_or_else(|| RecordError::Config("frame buffer too small".into()))?;
                (Pixel::RGB48LE, &hdr16[..], src.width as usize * 6, 6)
            }
        };
        if self.scaler.as_ref().is_none_or(|(_, k)| *k != key) {
            let mut ctx = scaling::Context::get(
                input_pix,
                width,
                height,
                self.pix,
                ow,
                oh,
                scaling::Flags::BICUBIC,
            )?;
            let matrix = if self.cfg.hdr {
                ffi::SWS_CS_BT2020
            } else {
                ffi::SWS_CS_ITU709
            };
            // SAFETY: `ctx` is a live swscale context; the coefficient tables are static.
            // Source RGB is full range, the output limited range, as the stream is tagged.
            unsafe {
                let table = ffi::sws_getCoefficients(matrix);
                ffi::sws_setColorspaceDetails(
                    ctx.as_mut_ptr(),
                    table,
                    1,
                    table,
                    0,
                    0,
                    1 << 16,
                    1 << 16,
                );
            }
            self.scaler = Some((ctx, key));
        }
        let mut input = frame::Video::new(input_pix, width, height);
        let dst_stride = input.stride(0);
        let row = width as usize * bpp;
        for y in 0..height as usize {
            let from = (y + top as usize) * stride + left as usize * bpp;
            let line = bytes
                .get(from..from + row)
                .ok_or_else(|| RecordError::Config("frame buffer too small".into()))?;
            input.data_mut(0)[y * dst_stride..y * dst_stride + row].copy_from_slice(line);
        }
        let mut output = frame::Video::new(self.pix, ow, oh);
        if let Some((ctx, _)) = &mut self.scaler {
            ctx.run(&input, &mut output)?;
        }
        Ok(output)
    }

    fn send(&mut self, index: u64) -> Result<(), RecordError> {
        match &mut self.last {
            None => return Ok(()),
            Some(Last::Sw(frame)) => {
                frame.set_pts(Some(index as i64));
                self.encoder.send_frame(frame)?;
            }
            Some(Last::Hw(frame)) => {
                // SAFETY: the frame is a live D3D11 `AVFrame`; the encoder takes its own
                // reference to the pixels, so the same frame can be sent again for a repeat.
                let code = unsafe {
                    (*frame.as_ptr()).pts = index as i64;
                    ffi::avcodec_send_frame(self.encoder.as_mut_ptr(), frame.as_ptr())
                };
                if code < 0 {
                    return Err(RecordError::Ffmpeg(format!("avcodec_send_frame: {code}")));
                }
            }
        }
        self.stats.encoded.fetch_add(1, Ordering::Relaxed);
        self.drain()
    }

    fn drain(&mut self) -> Result<(), RecordError> {
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            self.write(&mut packet)?;
        }
        Ok(())
    }

    fn split_due(&self, out: &Output, packet: &Packet) -> bool {
        if !packet.is_key() || out.packets == 0 {
            return false;
        }
        match self.cfg.split {
            Split::Off => false,
            Split::Bytes(limit) => out.bytes >= limit,
            Split::Duration(limit) => {
                let elapsed = packet.pts().unwrap_or(0) - out.first_pts;
                let seconds = elapsed as f64 * f64::from(self.enc_tb.0) / f64::from(self.enc_tb.1);
                seconds >= limit.as_secs_f64()
            }
        }
    }

    fn write(&mut self, packet: &mut Packet) -> Result<(), RecordError> {
        let video_pts = packet.pts().unwrap_or(0);
        let reached = self.pts_to_samples(video_pts);
        if let Some(ring) = &mut self.ring {
            ring.push_video(reached, packet);
        }
        let due = self.out.as_ref().is_some_and(|o| self.split_due(o, packet));
        if due {
            // The audio before the cut belongs to the part that ends.
            let cut = self.pts_to_samples(packet.pts().unwrap_or(0));
            self.write_audio_before(cut - 1)?;
            self.close_part()?;
            self.part += 1;
            let path = (self.namer)(self.part);
            let octx = format::output_as(&path, self.cfg.container.muxer())?;
            self.begin_part(octx, path)?;
        }
        let enc_tb = self.enc_tb;
        if let Some(out) = &mut self.out {
            let offset = *out.offset.get_or_insert_with(|| {
                out.first_pts = packet.pts().unwrap_or(0);
                out.first_pts
            });
            let first_pts = out.first_pts;
            let size = packet.size();
            packet.set_pts(packet.pts().map(|p| p - offset));
            packet.set_dts(packet.dts().map(|d| d - offset));
            packet.rescale_ts(self.enc_tb, out.stream_tb);
            packet.set_stream(0);
            packet.write_interleaved(&mut out.octx)?;
            out.packets += 1;
            out.bytes += size as u64;
            if out.packets == 1 {
                out.audio_cut = samples_of(enc_tb, first_pts);
            }
        }
        self.horizon = self.horizon.max(reached);
        self.write_ready_audio()
    }

    /// Writes the trailer; a hybrid MP4 is then rewritten with `moov` in front, without
    /// re-encoding.
    fn close_part(&mut self) -> Result<(), RecordError> {
        let Some(mut out) = self.out.take() else {
            return Ok(());
        };
        out.octx.write_trailer()?;
        let path = out.path.clone();
        drop(out);
        if self.cfg.container == OutputContainer::Mp4Hybrid {
            faststart(&path)?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Summary, RecordError> {
        self.encoder.send_eof()?;
        self.drain()?;
        self.finish_audio()?;
        self.close_part()?;
        Ok(Summary {
            files: self.files,
            frames: if self.cfg.vfr {
                self.stats.encoded.load(Ordering::Relaxed)
            } else {
                self.cfr.emitted()
            },
            repeated: self.cfr.repeated,
            dropped_by_clock: self.cfr.dropped,
            dropped_by_queue: self.stats.queue_dropped.load(Ordering::Relaxed),
        })
    }
}

/// Writes a replay: the packets of `snapshot`, video and audio merged by time, each stream
/// starting at zero.
fn write_snapshot(
    out: &mut Output,
    snapshot: &Snapshot,
    enc_tb: Rational,
) -> Result<(), RecordError> {
    // The first packet is a key frame; its dts may be before its pts (encoder delay), as in a
    // normal recording.
    let offset = snapshot
        .video
        .first()
        .and_then(|(_, p)| p.pts())
        .unwrap_or(0);
    let cut = samples_of(enc_tb, offset);
    let mut next = vec![0_usize; snapshot.audio.len()];
    let mut video = snapshot.video.iter().peekable();
    loop {
        // The earliest of the next video packet and the next packet of every audio track.
        let video_t = video.peek().map(|(t, _)| *t);
        let audio_pick = snapshot
            .audio
            .iter()
            .enumerate()
            .filter_map(|(i, q)| q.get(next[i]).map(|(t, _)| (*t, i)))
            .min();
        match (video_t, audio_pick) {
            (None, None) => break,
            (Some(vt), audio) if audio.is_none_or(|(at, _)| vt <= at) => {
                let Some((_, original)) = video.next() else {
                    break;
                };
                let mut packet = original
                    .load()
                    .map_err(|e| RecordError::Io(e.to_string()))?;
                packet.set_pts(packet.pts().map(|p| p - offset));
                packet.set_dts(packet.dts().map(|d| d - offset));
                packet.rescale_ts(enc_tb, out.stream_tb);
                packet.set_stream(0);
                packet.write_interleaved(&mut out.octx)?;
            }
            (_, Some((_, track))) => {
                let (_, original) = &snapshot.audio[track][next[track]];
                next[track] += 1;
                let mut packet = original
                    .load()
                    .map_err(|e| RecordError::Io(e.to_string()))?;
                let Some(pts) = packet.pts().map(|p| p - cut) else {
                    continue;
                };
                if pts < 0 {
                    continue; // starts before the key frame
                }
                packet.set_pts(Some(pts));
                packet.set_dts(packet.dts().map(|d| d - cut));
                let tb = out.audio_tb.get(track).copied().unwrap_or(enc_tb);
                packet.rescale_ts(Rational(1, SAMPLE_RATE as i32), tb);
                packet.set_stream(1 + track);
                packet.write_interleaved(&mut out.octx)?;
            }
            (Some(_), None) => unreachable!("handled by the first arm"),
        }
    }
    Ok(())
}

/// Rewrites a fragmented MP4 as a classic one with the `moov` box first (packet copy).
pub fn faststart(path: &Path) -> Result<(), RecordError> {
    let tmp = path.with_extension("faststart.tmp");
    let result = (|| -> Result<(), RecordError> {
        let mut input = format::input(path)?;
        let mut output = format::output_as(&tmp, "mp4")?;
        let mut map = Vec::new();
        for stream in input.streams() {
            let mut out = output.add_stream(encoder::find(codec::Id::None))?;
            out.set_parameters(stream.parameters());
            // SAFETY: the parameters belong to the stream just created; a zero tag lets the MP4
            // muxer choose its own.
            unsafe { (*out.parameters().as_mut_ptr()).codec_tag = 0 };
            map.push(out.index());
        }
        output.set_metadata(input.metadata().to_owned());
        let mut options = Dictionary::new();
        options.set("movflags", "faststart");
        output.write_header_with(options)?;
        for (stream, mut packet) in input.packets() {
            let Some(&out_index) = map.get(stream.index()) else {
                continue;
            };
            let Some(out_tb) = output.stream(out_index).map(|s| s.time_base()) else {
                continue;
            };
            packet.rescale_ts(stream.time_base(), out_tb);
            packet.set_position(-1);
            packet.set_stream(out_index);
            packet.write_interleaved(&mut output)?;
        }
        output.write_trailer()?;
        Ok(())
    })();
    match result {
        Ok(()) => std::fs::rename(&tmp, path).map_err(|e| RecordError::Io(e.to_string())),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
