// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording pipeline (plan 4.4, 5.9): frames with a monotonic timestamp go through the
//! constant-frame-rate clock ([`crate::clock`]), colour conversion and scaling, an encoder opened
//! from the registry, and a muxer (MKV, hybrid or fragmented MP4, WebM) that can split the output
//! on key frames. Everything runs on a dedicated worker thread behind a bounded queue, so a slow
//! encoder drops input frames (counted) instead of stalling the capture.
//!
//! This is the CPU path: frames come as BGRA in RAM and hardware encoders are fed from system
//! memory. The GPU path (Windows: D3D11 frames straight to NVENC/AMF/QSV) plugs in above this.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling;
use ffmpeg_next::{Dictionary, Packet, Rational, codec, color, encoder, ffi, format, frame};

use crate::clock::{Cfr, Emit, Fps};
use crate::registry::{Chroma, Encoder};

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

enum Cmd {
    Frame(i64, Box<VideoFrame>),
    Tick(i64),
    Pause(i64),
    Resume(i64),
    Stop(i64),
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
        match self.tx.try_send(Cmd::Frame(t, Box::new(frame))) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.stats.queue_dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
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
    last: Option<frame::Video>,
    out: Option<Output>,
    part: u32,
    files: Vec<PathBuf>,
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
        let path0 = namer(0);
        if let Some(dir) = path0.parent() {
            std::fs::create_dir_all(dir).map_err(|e| RecordError::Io(e.to_string()))?;
        }
        let octx = format::output_as(&path0, cfg.container.muxer())?;

        let mut ctx = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        ctx.set_width(w);
        ctx.set_height(h);
        ctx.set_format(pix);
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
        if octx.format().flags().contains(format::Flags::GLOBAL_HEADER) {
            flags |= codec::Flags::GLOBAL_HEADER;
        }
        ctx.set_flags(flags);
        let encoder = ctx.open_with(dictionary(&cfg.options))?;

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
            last: None,
            out: None,
            part: 0,
            files: Vec::new(),
        };
        worker.begin_part(octx, path0)?;
        Ok(worker)
    }

    fn begin_part(
        &mut self,
        mut octx: format::context::Output,
        path: PathBuf,
    ) -> Result<(), RecordError> {
        let mut stream = octx.add_stream(self.codec)?;
        stream.set_parameters(&self.encoder);
        octx.write_header_with(self.cfg.container.header_options())?;
        let stream_tb = octx.stream(0).map_or(self.enc_tb, |s| s.time_base());
        self.files.push(path.clone());
        self.out = Some(Output {
            octx,
            path,
            stream_tb,
            offset: None,
            first_pts: 0,
            packets: 0,
            bytes: 0,
        });
        Ok(())
    }

    fn run(mut self, rx: &Receiver<Cmd>) -> Result<Summary, RecordError> {
        let mut stop_at = None;
        while let Ok(cmd) = rx.recv() {
            let emits = match cmd {
                Cmd::Frame(t, frame) if self.cfg.vfr => {
                    if let Some(pts) = self.vfr.stamp(t) {
                        let converted = self.convert(&frame)?;
                        self.last = Some(converted);
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
                    let emits = self.cfr.on_frame(t);
                    self.emit(&emits, Some(&frame))?;
                    continue;
                }
                Cmd::Tick(t) => self.cfr.on_tick(t),
                Cmd::Pause(t) => self.cfr.pause(t),
                Cmd::Resume(t) => {
                    self.cfr.resume(t);
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
    fn emit(&mut self, emits: &[Emit], new: Option<&VideoFrame>) -> Result<(), RecordError> {
        for e in emits {
            match (*e, new) {
                (Emit::New(i), Some(frame)) => {
                    let converted = self.convert(frame)?;
                    self.last = Some(converted);
                    self.send(i)?;
                }
                (Emit::Repeat(i), _) => self.send(i)?,
                (Emit::New(_), None) => {}
            }
        }
        Ok(())
    }

    fn convert(&mut self, src: &VideoFrame) -> Result<frame::Video, RecordError> {
        let (ow, oh) = self.cfg.output_size;
        let key = (src.width, src.height, src.format);
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
                src.width,
                src.height,
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
                #[allow(clippy::unnecessary_cast)] // the constant's type differs between platforms
                let table = ffi::sws_getCoefficients(matrix as i32);
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
        let mut input = frame::Video::new(input_pix, src.width, src.height);
        let dst_stride = input.stride(0);
        let row = src.width as usize * bpp;
        for y in 0..src.height as usize {
            let from = y * stride;
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
        let Some(frame) = &mut self.last else {
            return Ok(());
        };
        frame.set_pts(Some(index as i64));
        self.encoder.send_frame(frame)?;
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
        let due = self.out.as_ref().is_some_and(|o| self.split_due(o, packet));
        if due {
            self.close_part()?;
            self.part += 1;
            let path = (self.namer)(self.part);
            let octx = format::output_as(&path, self.cfg.container.muxer())?;
            self.begin_part(octx, path)?;
        }
        let Some(out) = &mut self.out else {
            return Ok(());
        };
        let offset = *out.offset.get_or_insert_with(|| {
            out.first_pts = packet.pts().unwrap_or(0);
            out.first_pts
        });
        let size = packet.size();
        packet.set_pts(packet.pts().map(|p| p - offset));
        packet.set_dts(packet.dts().map(|d| d - offset));
        packet.rescale_ts(self.enc_tb, out.stream_tb);
        packet.set_stream(0);
        packet.write_interleaved(&mut out.octx)?;
        out.packets += 1;
        out.bytes += size as u64;
        Ok(())
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
