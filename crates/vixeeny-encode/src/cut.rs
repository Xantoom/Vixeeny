// SPDX-License-Identifier: GPL-3.0-or-later
//! Cutting a video to the frame: what the video editor of the replays plays and writes.
//!
//! A [`Clip`] lists the frames of a file (their time and which are key frames) without decoding
//! anything. [`Pictures`] decodes them for the preview (on the graphics card when it can), at the
//! size they are shown and in SDR (an HDR clip is tone mapped), and
//! [`Sound`] the audio tracks, mixed, for the speakers. [`export`] writes a part of the file:
//! the packets as they are when the part starts on a key frame (instant, nothing lost), else
//! the video encoded again with the encoder of the replay and the audio copied.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::resampling;
use ffmpeg_next::util::channel_layout::ChannelLayout;
use ffmpeg_next::util::format::sample::{Sample, Type as SampleType};
use ffmpeg_next::{Dictionary, Packet, Rational, codec, color, encoder, ffi, format, frame, media};
use vixeeny_common::colour;

use crate::hdr::Hdr10;
use crate::recorder::RecordError;
use crate::registry::{Chroma, Encoder};

/// The rate and layout of the sound handed to the speakers: 48 kHz, stereo, 32-bit floats.
pub const SOUND_RATE: u32 = 48_000;
pub const SOUND_CHANNELS: usize = 2;

/// One frame of the clip, in the order it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Presentation time, in the video stream's time base.
    pub pts: i64,
    pub key: bool,
    /// Its place in the file (decoding order).
    pub decode: usize,
}

/// What a video file holds, as far as cutting it goes.
#[derive(Debug, Clone)]
pub struct Clip {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Frames per second, as a fraction.
    pub rate: Rational,
    /// The time base of the video stream.
    pub time_base: Rational,
    /// Every frame, in presentation order.
    pub frames: Vec<Frame>,
    pub audio_tracks: usize,
    /// BT.2020 / PQ (HDR10).
    pub hdr: bool,
    /// More than 8 bits per component.
    pub deep: bool,
    /// The HDR10 metadata a re-encode carries on, when `hdr`.
    hdr10: Option<Hdr10>,
    /// SDR white of an HDR clip, in nits: the recorded monitor's when the file says it.
    pub sdr_white_nits: f32,
    video: usize,
}

/// How many packets of the video tell whether its frames are shown in the order they are
/// decoded (an encoder that reorders, with B-frames, does it from the first ones).
const ORDER_CHECKED: usize = 64;

/// The video's frames as the file's index lists them (an MP4's sample tables, read with the
/// file: instant, where reading every packet takes about 4 ms per second of 4K video), when it
/// lists them all and the frames are shown in the order they are decoded; `None` otherwise (a Matroska
/// file, a recording cut short, B-frames), and the packets are read.
fn indexed_frames(input: &mut format::context::Input, video: usize) -> Option<Vec<Frame>> {
    // SAFETY: the stream belongs to the live input; the entries are read while it is.
    let indexed = unsafe {
        let stream = *(*input.as_ptr()).streams.add(video);
        let count = ffi::avformat_index_get_entries_count(stream);
        if count <= 0 || i64::from(count) != (*stream).nb_frames {
            return None;
        }
        let mut frames = Vec::with_capacity(count as usize);
        for i in 0..count {
            let entry = ffi::avformat_index_get_entry(stream, i).as_ref()?;
            // An edit list hides frames: what is shown is then not what is listed.
            if entry.flags() & ffi::AVINDEX_DISCARD_FRAME != 0 {
                return None;
            }
            frames.push(Frame {
                pts: entry.timestamp,
                key: entry.flags() & ffi::AVINDEX_KEYFRAME != 0,
                decode: i as usize,
            });
        }
        frames
    };
    // The index holds decoding times. Frames shown in the order they are decoded are shown a
    // fixed time after it (zero, or the delay an encoder keeps); the first packets tell.
    let mut offset = None;
    let shown_as_decoded = input
        .packets()
        .filter(|(s, _)| s.index() == video)
        .take(ORDER_CHECKED)
        .zip(&indexed)
        .all(|((_, packet), frame)| match (packet.pts(), packet.dts()) {
            (Some(pts), Some(dts)) if dts == frame.pts => {
                *offset.get_or_insert(pts - dts) == pts - dts
            }
            _ => false,
        });
    let offset = offset.filter(|_| shown_as_decoded)?;
    let mut frames = indexed;
    for frame in &mut frames {
        frame.pts += offset;
    }
    Some(frames)
}

/// The video's frames, from every packet of the file.
fn scanned_frames(input: &mut format::context::Input, video: usize) -> Vec<Frame> {
    let mut frames = Vec::new();
    for (s, packet) in input.packets() {
        if s.index() != video {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            continue;
        };
        frames.push(Frame {
            pts,
            key: packet.is_key(),
            decode: frames.len(),
        });
    }
    frames
}

/// `pts` in seconds.
fn seconds(pts: i64, tb: Rational) -> f64 {
    pts as f64 * f64::from(tb.0) / f64::from(tb.1.max(1))
}

impl Clip {
    /// Reads the list of frames (the packets only: nothing is decoded).
    pub fn open(path: &Path) -> Result<Self, RecordError> {
        ffmpeg_next::init()?;
        crate::ffmpeg_log::install();
        let mut input = format::input(path)?;
        let stream = input
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| RecordError::Config("the file has no video".into()))?;
        let video = stream.index();
        let time_base = stream.time_base();
        let rate = stream.avg_frame_rate();
        let params = stream.parameters();
        // SAFETY: the parameters belong to the stream, alive while `input` is.
        let (width, height, hdr, deep, hdr10) = unsafe {
            let p = &*params.as_ptr();
            let deep =
                ffi::av_pix_fmt_desc_get(std::mem::transmute::<i32, ffi::AVPixelFormat>(p.format))
                    .as_ref()
                    .is_some_and(|d| d.comp[0].depth > 8);
            let hdr = p.color_trc == ffi::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084;
            (
                p.width.max(0) as u32,
                p.height.max(0) as u32,
                hdr,
                deep,
                hdr.then(|| Hdr10::of_stream(p)),
            )
        };
        let audio_tracks = input
            .streams()
            .filter(|s| s.parameters().medium() == media::Type::Audio)
            .count();
        let sdr_white_nits = input
            .metadata()
            .get(crate::hdr::SDR_WHITE_TAG)
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| (1.0..=10_000.0).contains(v))
            .unwrap_or(REFERENCE_WHITE);
        let mut frames = match indexed_frames(&mut input, video) {
            Some(frames) => frames,
            // Read again from the start: the index's check may have read some packets.
            None => scanned_frames(&mut format::input(path)?, video),
        };
        frames.sort_by_key(|f| f.pts);
        if frames.is_empty() {
            return Err(RecordError::Config("the file has no frames".into()));
        }
        Ok(Self {
            path: path.to_owned(),
            width,
            height,
            rate: if rate.0 > 0 && rate.1 > 0 {
                rate
            } else {
                Rational(60, 1)
            },
            time_base,
            frames,
            audio_tracks,
            hdr,
            deep,
            hdr10,
            sdr_white_nits,
            video,
        })
    }

    /// Whether the frames of `path` are read from its index rather than its packets (tests).
    #[doc(hidden)]
    pub fn listed_by_index(path: &Path) -> Result<bool, RecordError> {
        let mut input = format::input(path)?;
        let video = input
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| RecordError::Config("the file has no video".into()))?
            .index();
        Ok(indexed_frames(&mut input, video).is_some())
    }

    /// When frame `i` is shown, in seconds from the first frame.
    pub fn time(&self, i: usize) -> f64 {
        let first = self.frames.first().map_or(0, |f| f.pts);
        let pts = self.frames.get(i).map_or(first, |f| f.pts);
        seconds(pts - first, self.time_base)
    }

    /// How long one frame lasts, in seconds.
    pub fn frame_seconds(&self) -> f64 {
        f64::from(self.rate.1) / f64::from(self.rate.0)
    }

    /// The length of the clip, its last frame included.
    pub fn duration(&self) -> f64 {
        self.time(self.frames.len().saturating_sub(1)) + self.frame_seconds()
    }

    /// The key frame at or before frame `i`.
    pub fn key_before(&self, i: usize) -> usize {
        (0..=i.min(self.frames.len().saturating_sub(1)))
            .rev()
            .find(|&k| self.frames[k].key)
            .unwrap_or(0)
    }

    /// Whether frames `from..=to` can be written as they are: the part starts on a key frame
    /// and its packets follow each other in the file, with no other frame among them.
    pub fn lossless(&self, from: usize, to: usize) -> bool {
        let Some(part) = self.frames.get(from..=to) else {
            return false;
        };
        let first = part.iter().map(|f| f.decode).min();
        let last = part.iter().map(|f| f.decode).max();
        part[0].key
            && first == Some(part[0].decode)
            && last
                .zip(first)
                .is_some_and(|(l, f)| l - f + 1 == part.len())
    }

    /// The index of the frame whose time is `pts`, if one is.
    fn index_of(&self, pts: i64) -> Option<usize> {
        self.frames.binary_search_by_key(&pts, |f| f.pts).ok()
    }
}

/// A frame scaled for the screen: `width`×`height` BGRA pixels, rows packed.
#[derive(Debug, Clone)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Scales decoded frames to BGRA of one size, on every core; an HDR frame is tone mapped.
/// Writes into the buffer it is given (no picture allocated per frame).
struct Scaler {
    ctx: *mut ffi::SwsContext,
    /// For an HDR clip (BT.2020, PQ).
    tone: Option<ToneMap>,
    /// The 16-bit RGB picture an HDR frame goes through, kept for the next frame.
    deep: Option<frame::Video>,
}

// SAFETY: used through `&mut self` only, by one thread at a time.
unsafe impl Send for Scaler {}

impl Scaler {
    /// `tone`: for the frames of an HDR clip shown in SDR.
    fn new(tone: Option<ToneMap>) -> Result<Self, RecordError> {
        // SAFETY: plain allocation, checked for NULL.
        let ctx = unsafe { ffi::sws_alloc_context() };
        if ctx.is_null() {
            return Err(RecordError::Config("cannot allocate a scaler".into()));
        }
        // SAFETY: a live context; NUL-terminated option names.
        unsafe {
            ffi::av_opt_set_int(ctx.cast(), c"threads".as_ptr(), 0, 0);
            ffi::av_opt_set_int(
                ctx.cast(),
                c"sws_flags".as_ptr(),
                i64::from(ffi::SwsFlags::SWS_BILINEAR as u32),
                0,
            );
        }
        Ok(Self {
            ctx,
            tone,
            deep: None,
        })
    }

    /// Scales `src` into `dst`, whose buffers are already there (a frame of the size and
    /// format wanted): the scaler writes into them.
    fn scale_into(
        &mut self,
        dst: &mut frame::Video,
        src: &frame::Video,
    ) -> Result<(), RecordError> {
        // SAFETY: both frames are live and `dst` has its buffers (`data[0]` set), which the
        // scaler fills ("user-provided buffers") without taking them.
        let code = unsafe { ffi::sws_scale_frame(self.ctx, dst.as_mut_ptr(), src.as_ptr()) };
        if code < 0 {
            return Err(ffmpeg_next::Error::from(code).into());
        }
        Ok(())
    }

    /// `src` as `size` BGRA, in `bgra` (its memory reused when it is large enough).
    fn run(
        &mut self,
        src: &frame::Video,
        size: (u32, u32),
        mut bgra: Vec<u8>,
    ) -> Result<Picture, RecordError> {
        let (w, h) = (size.0.max(2), size.1.max(2));
        let len = w as usize * h as usize * 4;
        bgra.clear();
        bgra.resize(len, 0);
        if self.tone.is_some() {
            // To 16-bit RGB, still BT.2020 and PQ (nothing converted but the matrix), then
            // tone mapped here.
            let fits = self
                .deep
                .as_ref()
                .is_some_and(|d| (d.width(), d.height()) == (w, h));
            if !fits {
                let mut deep = frame::Video::new(Pixel::RGB48LE, w, h);
                // SAFETY: a live frame; plain fields.
                unsafe {
                    let o = &mut *deep.as_mut_ptr();
                    o.colorspace = ffi::AVColorSpace::AVCOL_SPC_RGB;
                    o.color_range = ffi::AVColorRange::AVCOL_RANGE_JPEG;
                    o.color_primaries = ffi::AVColorPrimaries::AVCOL_PRI_BT2020;
                    o.color_trc = ffi::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084;
                }
                self.deep = Some(deep);
            }
            let Some(mut deep) = self.deep.take() else {
                return Err(RecordError::Config("no HDR picture".into()));
            };
            let scaled = self.scale_into(&mut deep, src);
            if let (Ok(()), Some(tone)) = (&scaled, &self.tone) {
                tone.run(
                    deep.data(0),
                    deep.stride(0),
                    w as usize,
                    h as usize,
                    &mut bgra,
                );
            }
            self.deep = Some(deep);
            scaled?;
        } else {
            // A frame over `bgra` itself: the scaler writes the picture there.
            let mut out = frame::Video::empty();
            // SAFETY: a frame of no buffers of its own, pointing at `bgra` (alive and of
            // `len` bytes until the call returns); its pointers are cleared before it is
            // dropped, so nothing frees `bgra`.
            unsafe {
                let o = &mut *out.as_mut_ptr();
                o.format = ffi::AVPixelFormat::AV_PIX_FMT_BGRA as i32;
                o.width = w as i32;
                o.height = h as i32;
                o.data[0] = bgra.as_mut_ptr();
                o.linesize[0] = (w * 4) as i32;
            }
            let scaled = self.scale_into(&mut out, src);
            // SAFETY: as above.
            unsafe {
                let o = &mut *out.as_mut_ptr();
                o.data[0] = std::ptr::null_mut();
                o.linesize[0] = 0;
            }
            scaled?;
        }
        Ok(Picture {
            width: w,
            height: h,
            bgra,
        })
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: allocated by `sws_alloc_context`; set to NULL by the call.
        unsafe { ffi::sws_free_context(&raw mut self.ctx) };
    }
}

/// HDR (BT.2020 primaries, PQ) to SDR (BT.709, sRGB) for the preview and the animated exports,
/// as the screenshots and the recordings are tone mapped (`colour`): the clip's SDR white (the
/// recorded monitor's, tagged by the recorder; else the BT.2408 reference, 203 nits) becomes
/// SDR white, and what is brighter rolls off up to the brightest pixel of the frame, followed
/// smoothly from frame to frame and never past the clip's peak. Colours keep their hue, and
/// those wider than BT.709 are desaturated just enough to fit.
pub struct ToneMap {
    /// PQ code (12 bits) → linear light, 1.0 at the clip's SDR white.
    linear: Vec<f32>,
    /// Linear 0..1 (16 bits) → sRGB 8 bits.
    srgb: Vec<u8>,
    /// The clip's peak, in units of its SDR white.
    display: f32,
    steps: (f32, f32),
    /// The headroom followed (the bits of an `f32`; 0: none yet).
    followed: AtomicU32,
}

/// The reference white of an HDR clip that does not say its own, nits (BT.2408).
pub const REFERENCE_WHITE: f32 = 203.0;

/// BT.2020 to BT.709, linear light (ITU-R BT.2087).
const BT2020_TO_709: [[f32; 3]; 3] = [
    [1.660_491, -0.587_641, -0.072_850],
    [-0.124_550, 1.132_9, -0.008_349],
    [-0.018_151, -0.100_579, 1.118_73],
];

impl ToneMap {
    /// For a clip whose SDR white is `white_nits` and whose brightest pixel is `peak_nits`,
    /// played at `fps`.
    pub fn new(white_nits: f32, peak_nits: f32, fps: f64) -> Self {
        let (m1, m2) = (0.159_301_757_812_5, 78.843_75);
        let (c1, c2, c3) = (0.835_937_5, 18.851_562_5, 18.687_5);
        let white = f64::from(white_nits.max(1.0));
        let linear = (0..4096)
            .map(|i| {
                let e = (f64::from(i) / 4095.0).powf(1.0 / m2);
                let nits = ((e - c1).max(0.0) / (c2 - c3 * e)).powf(1.0 / m1) * 10_000.0;
                (nits / white) as f32
            })
            .collect();
        let srgb = (0..=u16::MAX)
            .map(|i| (colour::srgb_encode(f32::from(i) / 65535.0) * 255.0).round() as u8)
            .collect();
        Self {
            linear,
            srgb,
            display: (f64::from(peak_nits) / white) as f32,
            steps: colour::follow_steps(fps),
            followed: AtomicU32::new(0),
        }
    }

    /// The tone mapping of `clip` when it is HDR: its SDR white, its peak, its frame rate.
    fn of(clip: &Clip) -> Option<Self> {
        clip.hdr.then(|| {
            let peak = clip.hdr10.map_or(1000.0, |m| m.peak_nits());
            let fps = f64::from(clip.rate.0) / f64::from(clip.rate.1.max(1));
            Self::new(clip.sdr_white_nits, peak, fps)
        })
    }

    /// Forgets the headroom followed: the next frame takes its own (after a seek).
    pub fn reset(&self) {
        self.followed.store(0, Ordering::Relaxed);
    }

    /// PQ codes (16 bits, full range) as linear BT.709, inside its gamut, 1.0 at SDR white.
    fn light(&self, rgb: [u16; 3]) -> [f32; 3] {
        let lin = rgb.map(|c| self.linear[usize::from(c >> 4)]);
        colour::into_gamut(
            BT2020_TO_709.map(|row| row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]),
        )
    }

    /// One pixel: PQ codes (16 bits, full range) to sRGB BGRA, rolled off up to `headroom`.
    pub fn pixel(&self, rgb: [u16; 3], headroom: f32) -> [u8; 4] {
        let c = self.light(rgb);
        let y = colour::luminance(c);
        // Black, or not a number.
        if y.is_nan() || y <= f32::EPSILON {
            return [0, 0, 0, 255];
        }
        let k = colour::roll_off(y, headroom) / y;
        let mut o = c.map(|v| v * k);
        let m = o[0].max(o[1]).max(o[2]);
        if m > 1.0 {
            o = o.map(|v| v / m);
        }
        let code = |v: f32| self.srgb[(v.clamp(0.0, 1.0) * 65535.0 + 0.5) as usize];
        [code(o[2]), code(o[1]), code(o[0]), 255]
    }

    /// A picture of `w`×`h` RGB48 (rows `stride` bytes apart) to packed BGRA in `bgra`, on
    /// every core.
    fn run(&self, data: &[u8], stride: usize, w: usize, h: usize, bgra: &mut [u8]) {
        self.run_with::<6>(data, stride, (w, h), bgra, |px| {
            let c = |i: usize| u16::from_le_bytes([px[i], px[i + 1]]);
            [c(0), c(2), c(4)]
        });
    }

    /// Like [`run`](Self::run), from pixels of `N` bytes whose PQ codes `codes` reads (16 bits).
    /// The frame is rolled off up to its brightest pixel, followed from the frames before.
    pub(crate) fn run_with<const N: usize>(
        &self,
        data: &[u8],
        stride: usize,
        (w, h): (usize, usize),
        bgra: &mut [u8],
        codes: impl Fn(&[u8; N]) -> [u16; 3] + Sync,
    ) {
        let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        let rows = h.div_ceil(threads).max(1);
        let codes = &codes;
        let pixels = |chunk: usize, r: usize| {
            data[(chunk * rows + r) * stride..][..w * N]
                .as_chunks::<N>()
                .0
                .iter()
        };
        let brightest = std::thread::scope(|s| {
            let parts: Vec<_> = (0..h.div_ceil(rows))
                .map(|chunk| {
                    s.spawn(move || {
                        let mut top = 0.0_f32;
                        for r in 0..rows.min(h - chunk * rows) {
                            for px in pixels(chunk, r) {
                                let y = colour::luminance(self.light(codes(px)));
                                if y.is_finite() {
                                    top = top.max(y);
                                }
                            }
                        }
                        top
                    })
                })
                .collect();
            parts
                .into_iter()
                .filter_map(|p| p.join().ok())
                .fold(0.0, f32::max)
        });
        let target = colour::headroom(brightest, self.display);
        let before = f32::from_bits(self.followed.load(Ordering::Relaxed));
        let headroom = colour::follow(before, target, self.steps);
        self.followed.store(headroom.to_bits(), Ordering::Relaxed);
        std::thread::scope(|s| {
            for (chunk, out) in bgra.chunks_mut(rows * w * 4).enumerate() {
                s.spawn(move || {
                    for (r, line) in out.chunks_exact_mut(w * 4).enumerate() {
                        for (px, dst) in pixels(chunk, r).zip(line.as_chunks_mut::<4>().0) {
                            *dst = self.pixel(codes(px), headroom);
                        }
                    }
                });
            }
        });
    }
}

/// A Direct3D 11 device for decoding, if the machine has one (a reference to hand over).
fn d3d11_device() -> Option<*mut ffi::AVBufferRef> {
    let mut device = std::ptr::null_mut();
    // SAFETY: an out-pointer; no device name or options.
    let code = unsafe {
        ffi::av_hwdevice_ctx_create(
            &raw mut device,
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        )
    };
    (code >= 0 && !device.is_null()).then_some(device)
}

/// Seeks `input` to the key frame at or before `pts` of `stream`.
fn seek(input: &mut format::context::Input, stream: usize, pts: i64) -> Result<(), RecordError> {
    // SAFETY: a live input context; the bounds ask for the last key frame not after `pts`.
    let code = unsafe {
        ffi::avformat_seek_file(input.as_mut_ptr(), stream as i32, i64::MIN, pts, pts, 0)
    };
    if code < 0 {
        return Err(ffmpeg_next::Error::from(code).into());
    }
    Ok(())
}

/// A video decoder: on the graphics card when `hardware` and the machine has one (FFmpeg keeps
/// to the processor for what the card cannot decode), else on every core (frame threads).
/// Returns whether it decodes on the card.
fn video_decoder(
    params: codec::Parameters,
    hardware: bool,
) -> Result<(ffmpeg_next::decoder::Video, bool), RecordError> {
    let mut context = codec::context::Context::from_parameters(params.clone())?;
    if hardware && let Some(device) = d3d11_device() {
        // SAFETY: the context owns the reference from now on (freed with it).
        unsafe { (*context.as_mut_ptr()).hw_device_ctx = device };
        // dav1d, FFmpeg's choice for AV1, decodes on the processor only.
        let codec = if context.id() == codec::Id::AV1 {
            ffmpeg_next::decoder::find_by_name("av1")
        } else {
            ffmpeg_next::decoder::find(context.id())
        };
        if let Some(codec) = codec
            && let Ok(decoder) = context
                .decoder()
                .open_as(codec)
                .and_then(ffmpeg_next::decoder::Opened::video)
        {
            return Ok((decoder, true));
        }
        context = codec::context::Context::from_parameters(params)?;
    }
    context.set_threading(codec::threading::Config {
        kind: codec::threading::Type::Frame,
        count: 0,
    });
    Ok((context.decoder().video()?, false))
}

/// Decodes the frames of a clip for the preview, scaled to the size they are shown at.
pub struct Pictures {
    clip: Clip,
    input: format::context::Input,
    decoder: ffmpeg_next::decoder::Video,
    /// The decoder works on the graphics card.
    hardware: bool,
    scaler: Scaler,
    /// Scales the frames of the card on the card; `None` once it failed (the processor
    /// scales them then).
    #[cfg(windows)]
    card: Option<crate::card_scale::CardScaler>,
    /// Frames before this one are decoded but not handed out (after a seek).
    from: usize,
    eof: bool,
    decoded: frame::Video,
    /// A frame of the card, copied to memory (its buffers kept from frame to frame).
    copied: frame::Video,
    /// Pictures handed back ([`Pictures::recycle`]), their memory used again.
    spare: Vec<Vec<u8>>,
}

/// What a read gives.
#[derive(Debug)]
pub enum Next<T> {
    Frame(T),
    /// The end of the clip.
    End,
    /// Given up for a newer request.
    Stopped,
}

/// Pictures handed back are kept up to this many.
const SPARE: usize = 8;

impl Pictures {
    /// Decodes on the graphics card when it can.
    pub fn open(clip: &Clip) -> Result<Self, RecordError> {
        Self::open_with(clip, true)
    }

    /// Decodes on the graphics card when `hardware` and it can, else on the processor.
    pub fn open_with(clip: &Clip, hardware: bool) -> Result<Self, RecordError> {
        let input = format::input(&clip.path)?;
        let params = input
            .stream(clip.video)
            .ok_or_else(|| RecordError::Config("no video".into()))?
            .parameters();
        let (decoder, hardware) = video_decoder(params, hardware)?;
        Ok(Self {
            clip: clip.clone(),
            decoder,
            hardware,
            input,
            scaler: Scaler::new(ToneMap::of(clip))?,
            #[cfg(windows)]
            card: Some(crate::card_scale::CardScaler::new()),
            from: 0,
            eof: false,
            decoded: frame::Video::empty(),
            copied: frame::Video::empty(),
            spare: Vec::new(),
        })
    }

    /// Hands back the pixels of a picture shown: the next ones are written there.
    pub fn recycle(&mut self, bgra: Vec<u8>) {
        if self.spare.len() < SPARE {
            self.spare.push(bgra);
        }
    }

    /// Whether the graphics card decodes.
    pub fn hardware(&self) -> bool {
        self.hardware
    }

    /// Whether the graphics card also scales what it decodes (only the pictures shown come
    /// back to memory).
    pub fn scaled_on_card(&self) -> bool {
        #[cfg(windows)]
        return self.hardware && self.card.is_some();
        #[cfg(not(windows))]
        false
    }

    /// The card failed (a codec or profile it has not): the processor takes over, from the
    /// frame it was at.
    fn fall_back_to_software(&mut self) -> Result<(), RecordError> {
        tracing::info!("video editor: decoding on the processor");
        let params = self
            .input
            .stream(self.clip.video)
            .ok_or_else(|| RecordError::Config("no video".into()))?
            .parameters();
        (self.decoder, self.hardware) = video_decoder(params, false)?;
        let from = self.from;
        self.seek_from_key(from)
    }

    /// The next [`Pictures::read`] gives frame `i`. A frame a little further on is reached by
    /// decoding on from where the decoder is, rather than from its key frame again.
    pub fn seek(&mut self, i: usize) -> Result<(), RecordError> {
        let i = i.min(self.clip.frames.len().saturating_sub(1));
        // No key frame between here and there: going on decodes no more than a seek would.
        if !self.eof && i >= self.from && self.clip.key_before(i) <= self.from {
            self.from = i;
            return Ok(());
        }
        self.seek_from_key(i)
    }

    /// Starts decoding again from the key frame before frame `i`, which the next read gives.
    fn seek_from_key(&mut self, i: usize) -> Result<(), RecordError> {
        let key = self.clip.frames[self.clip.key_before(i)].pts;
        seek(&mut self.input, self.clip.video, key)?;
        self.decoder.flush();
        if let Some(tone) = &self.scaler.tone {
            tone.reset();
        }
        self.from = i;
        self.eof = false;
        Ok(())
    }

    /// The next frame without scaling it. Frames before the last seek target are skipped, and
    /// `stop` is asked after each of them.
    fn next_decoded(&mut self, stop: &dyn Fn() -> bool) -> Result<Next<usize>, RecordError> {
        loop {
            if self.decoder.receive_frame(&mut self.decoded).is_ok() {
                let pts = self.decoded.timestamp().or(self.decoded.pts());
                let Some(i) = pts.and_then(|p| self.clip.index_of(p)) else {
                    continue;
                };
                if i < self.from {
                    if stop() {
                        return Ok(Next::Stopped);
                    }
                    continue;
                }
                self.from = i + 1;
                return Ok(Next::Frame(i));
            }
            if self.eof {
                return Ok(Next::End);
            }
            let mut sent = false;
            let mut failed = false;
            while let Some((s, packet)) = self.input.packets().next() {
                if s.index() == self.clip.video {
                    // A broken packet is skipped, as a player does; on the card, an error is
                    // the card's.
                    failed = self.decoder.send_packet(&packet).is_err() && self.hardware;
                    sent = true;
                    break;
                }
            }
            if failed {
                self.fall_back_to_software()?;
                continue;
            }
            if !sent {
                self.eof = true;
                let _ = self.decoder.send_eof();
            }
        }
    }

    /// The next frame and its index, scaled to `size`; `None` at the end.
    pub fn read(&mut self, size: (u32, u32)) -> Result<Option<(usize, Picture)>, RecordError> {
        Ok(match self.read_unless(size, &|| false)? {
            Next::Frame(frame) => Some(frame),
            Next::End | Next::Stopped => None,
        })
    }

    /// [`read`](Self::read), given up as soon as `stop` says so while it decodes the frames
    /// before a seek's target (a newer request came): the next seek goes on from there.
    pub fn read_unless(
        &mut self,
        size: (u32, u32),
        stop: &dyn Fn() -> bool,
    ) -> Result<Next<(usize, Picture)>, RecordError> {
        let i = match self.next_decoded(stop)? {
            Next::Frame(i) => i,
            Next::End => return Ok(Next::End),
            Next::Stopped => return Ok(Next::Stopped),
        };
        let bgra = self.spare.pop().unwrap_or_default();
        // SAFETY: a live frame; a plain field.
        let on_card = unsafe { !(*self.decoded.as_ptr()).hw_frames_ctx.is_null() };
        if !on_card {
            return Ok(Next::Frame((
                i,
                self.scaler.run(&self.decoded, size, bgra)?,
            )));
        }
        #[cfg(windows)]
        let bgra = match self.on_card(size, bgra) {
            Ok(picture) => return Ok(Next::Frame((i, picture))),
            Err(bgra) => bgra,
        };
        self.copy_from_card()?;
        Ok(Next::Frame((i, self.scaler.run(&self.copied, size, bgra)?)))
    }

    /// The frame of the card scaled on the card; `bgra` back when the card cannot.
    #[cfg(windows)]
    fn on_card(&mut self, size: (u32, u32), mut bgra: Vec<u8>) -> Result<Picture, Vec<u8>> {
        let Some(card) = self.card.as_mut() else {
            return Err(bgra);
        };
        let (w, h) = (size.0.max(2), size.1.max(2));
        bgra.clear();
        bgra.resize(w as usize * h as usize * 4, 0);
        // SAFETY: the frame was just decoded by the card (`hw_frames_ctx` checked by the caller).
        let done = unsafe {
            card.run(
                self.decoded.as_ptr(),
                (w, h),
                &mut bgra,
                self.scaler.tone.as_ref(),
            )
        };
        match done {
            Ok(()) => Ok(Picture {
                width: w,
                height: h,
                bgra,
            }),
            Err(e) => {
                tracing::info!("video editor: frames scaled on the processor ({e})");
                self.card = None;
                Err(bgra)
            }
        }
    }

    /// Copies the frame of the card to memory, into the buffers of the last copy when they
    /// fit (no allocation per frame).
    fn copy_from_card(&mut self) -> Result<(), RecordError> {
        // SAFETY: both frames are live; `hw_frames_ctx` of a card frame holds an
        // `AVHWFramesContext`. The copy gets buffers of its own once (format, size), then the
        // card's pixels are written into them.
        unsafe {
            let src = self.decoded.as_ptr();
            let dst = self.copied.as_mut_ptr();
            let frames = (*(*src).hw_frames_ctx)
                .data
                .cast::<ffi::AVHWFramesContext>();
            let format = (*frames).sw_format as i32;
            let fits = !(*dst).buf[0].is_null()
                && (*dst).format == format
                && (*dst).width == (*src).width
                && (*dst).height == (*src).height;
            if !fits {
                ffi::av_frame_unref(dst);
                (*dst).format = format;
                (*dst).width = (*src).width;
                (*dst).height = (*src).height;
                let code = ffi::av_frame_get_buffer(dst, 0);
                if code < 0 {
                    return Err(ffmpeg_next::Error::from(code).into());
                }
            }
            let code = ffi::av_hwframe_transfer_data(dst, src, 0);
            if code < 0 {
                return Err(ffmpeg_next::Error::from(code).into());
            }
            ffi::av_frame_copy_props(dst, src);
        }
        Ok(())
    }
}

/// The audio tracks of a clip, decoded, mixed and resampled for the speakers
/// ([`SOUND_RATE`], [`SOUND_CHANNELS`] interleaved floats).
pub struct Sound {
    input: format::context::Input,
    tracks: Vec<Track>,
    /// Time of the first video frame, in seconds: the clip's zero.
    origin: f64,
    /// Samples (per channel) before this time are dropped (after a seek).
    from: f64,
    eof: bool,
}

struct Track {
    stream: usize,
    time_base: Rational,
    decoder: ffmpeg_next::decoder::Audio,
    resampler: Option<resampling::Context>,
    /// Mixed-ready samples, interleaved.
    pending: Vec<f32>,
    done: bool,
}

impl Sound {
    /// `None` when the clip has no sound.
    pub fn open(clip: &Clip) -> Result<Option<Self>, RecordError> {
        let input = format::input(&clip.path)?;
        let mut tracks = Vec::new();
        for s in input.streams() {
            if s.parameters().medium() != media::Type::Audio {
                continue;
            }
            let context = codec::context::Context::from_parameters(s.parameters())?;
            tracks.push(Track {
                stream: s.index(),
                time_base: s.time_base(),
                decoder: context.decoder().audio()?,
                resampler: None,
                pending: Vec::new(),
                done: false,
            });
        }
        if tracks.is_empty() {
            return Ok(None);
        }
        let first = clip.frames.first().map_or(0, |f| f.pts);
        Ok(Some(Self {
            input,
            tracks,
            origin: seconds(first, clip.time_base),
            from: 0.0,
            eof: false,
        }))
    }

    /// The next [`Sound::read`] starts at `t` seconds of the clip.
    pub fn seek(&mut self, t: f64) -> Result<(), RecordError> {
        let target = self.origin + t;
        // AV_TIME_BASE units, any stream.
        let ts = (target * 1_000_000.0) as i64;
        // SAFETY: a live input context.
        let code =
            unsafe { ffi::avformat_seek_file(self.input.as_mut_ptr(), -1, i64::MIN, ts, ts, 0) };
        if code < 0 {
            return Err(ffmpeg_next::Error::from(code).into());
        }
        for track in &mut self.tracks {
            track.decoder.flush();
            track.resampler = None;
            track.pending.clear();
            track.done = false;
        }
        self.from = target;
        self.eof = false;
        Ok(())
    }

    /// The next mixed samples (interleaved, at least one frame), `None` at the end.
    pub fn read(&mut self) -> Result<Option<Vec<f32>>, RecordError> {
        loop {
            let ready = self
                .tracks
                .iter()
                .filter(|t| !t.done || !t.pending.is_empty())
                .map(|t| t.pending.len())
                .min();
            match ready {
                None => return Ok(None),
                Some(n) if n > 0 => return Ok(Some(self.mix(n))),
                Some(_) => {}
            }
            if self.eof {
                for track in &mut self.tracks {
                    if track.pending.is_empty() {
                        track.done = true;
                    }
                }
                continue;
            }
            self.read_packet()?;
        }
    }

    fn mix(&mut self, n: usize) -> Vec<f32> {
        let mut out = vec![0.0_f32; n];
        for track in &mut self.tracks {
            if track.pending.len() >= n {
                for (o, s) in out.iter_mut().zip(track.pending.drain(..n)) {
                    *o += s;
                }
            }
        }
        out
    }

    fn read_packet(&mut self) -> Result<(), RecordError> {
        let Some((s, packet)) = self.input.packets().next() else {
            self.eof = true;
            for track in &mut self.tracks {
                let _ = track.decoder.send_eof();
            }
            return self.drain();
        };
        let index = s.index();
        if let Some(track) = self.tracks.iter_mut().find(|t| t.stream == index) {
            let _ = track.decoder.send_packet(&packet);
        }
        self.drain()
    }

    /// Takes what the decoders have, resampled, into the tracks' queues.
    fn drain(&mut self) -> Result<(), RecordError> {
        let from = self.from;
        let mut decoded = frame::Audio::empty();
        for track in &mut self.tracks {
            while track.decoder.receive_frame(&mut decoded).is_ok() {
                let start = decoded
                    .timestamp()
                    .or(decoded.pts())
                    .map_or(from, |p| seconds(p, track.time_base));
                if track.resampler.is_none() {
                    let layout = if decoded.channel_layout().channels() > 0 {
                        decoded.channel_layout()
                    } else {
                        ChannelLayout::default(i32::from(decoded.channels()))
                    };
                    track.resampler = Some(resampling::Context::get(
                        decoded.format(),
                        layout,
                        decoded.rate(),
                        Sample::F32(SampleType::Packed),
                        ChannelLayout::STEREO,
                        SOUND_RATE,
                    )?);
                }
                let Some(resampler) = track.resampler.as_mut() else {
                    continue;
                };
                let mut out = frame::Audio::empty();
                resampler.run(&decoded, &mut out)?;
                let samples = out.samples() * SOUND_CHANNELS;
                let bytes = &out.data(0)[..samples * 4];
                // What comes before the seek target is dropped.
                let skip = if start < from {
                    let skip = ((from - start) * f64::from(SOUND_RATE)).round() as usize;
                    (skip * SOUND_CHANNELS).min(samples)
                } else {
                    0
                };
                track.pending.extend(
                    bytes.as_chunks::<4>().0[skip..]
                        .iter()
                        .map(|b| f32::from_ne_bytes(*b)),
                );
            }
        }
        Ok(())
    }
}

/// The encoder that writes a part which does not start on a key frame.
#[derive(Debug, Clone)]
pub struct Reencode {
    pub encoder: Encoder,
    /// Its FFmpeg options (the video settings' preset).
    pub options: Vec<(String, String)>,
    pub depth: u8,
}

/// Writes frames `from..=to` of `clip` to `dest`: the packets as they are when
/// [`Clip::lossless`], else the video encoded again by `reencode`. The audio is copied.
/// `progress` hears the share done (0 to 1); `cancel` stops it (the file is removed).
pub fn export(
    clip: &Clip,
    (from, to): (usize, usize),
    dest: &Path,
    reencode: Option<&Reencode>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(), RecordError> {
    if from > to || to >= clip.frames.len() {
        return Err(RecordError::Config("empty cut".into()));
    }
    let lossless = clip.lossless(from, to);
    if !lossless && reencode.is_none() {
        return Err(RecordError::Config("this cut needs an encoder".into()));
    }
    let tmp = dest.with_extension("part.tmp");
    let result = write_cut(
        clip,
        (from, to),
        &tmp,
        dest,
        (!lossless).then_some(reencode).flatten(),
        progress,
        cancel,
    );
    match result {
        Ok(()) => std::fs::rename(&tmp, dest).map_err(|e| RecordError::Io(e.to_string())),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// An encoder for the frames of `clip`, opened.
struct VideoOut {
    encoder: encoder::Video,
    scaler: Scaler,
    /// The picture handed to the encoder, reused once the encoder no longer holds it.
    frame: Option<frame::Video>,
    pix: Pixel,
    time_base: Rational,
    hdr10: Option<Hdr10>,
}

fn open_encoder(
    clip: &Clip,
    reencode: &Reencode,
    global_header: bool,
) -> Result<(VideoOut, ffmpeg_next::Codec), RecordError> {
    let pix = crate::recorder::pixel_format(&reencode.encoder, reencode.depth, Chroma::C420)?;
    let codec = encoder::find_by_name(&reencode.encoder.ffmpeg_encoder).ok_or_else(|| {
        RecordError::Config(format!(
            "{} is not built in",
            reencode.encoder.ffmpeg_encoder
        ))
    })?;
    let gop = (f64::from(clip.rate.0) / f64::from(clip.rate.1)).round() * 2.0;
    let encoder = crate::recorder::open_video(&reencode.options, |options| {
        let mut ctx = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        ctx.set_width(clip.width & !1);
        ctx.set_height(clip.height & !1);
        ctx.set_format(pix);
        ctx.set_time_base(clip.time_base);
        ctx.set_frame_rate(Some(clip.rate));
        ctx.set_gop(gop.max(1.0) as u32);
        ctx.set_color_range(color::Range::MPEG);
        if clip.hdr {
            ctx.set_colorspace(color::Space::BT2020NCL);
            ctx.set_color_primaries(color::Primaries::BT2020);
            ctx.set_color_transfer_characteristic(color::TransferCharacteristic::SMPTE2084);
            if let Some(meta) = clip.hdr10 {
                // SAFETY: the context is live and not open yet.
                unsafe { meta.attach_to_context(ctx.as_mut_ptr()) };
            }
        } else {
            ctx.set_colorspace(color::Space::BT709);
            ctx.set_color_primaries(color::Primaries::BT709);
            ctx.set_color_transfer_characteristic(color::TransferCharacteristic::BT709);
        }
        let mut flags = codec::Flags::CLOSED_GOP;
        if global_header {
            flags |= codec::Flags::GLOBAL_HEADER;
        }
        ctx.set_flags(flags);
        Ok(ctx.open_with(options)?)
    })?;
    Ok((
        VideoOut {
            encoder,
            scaler: Scaler::new(None)?,
            frame: None,
            pix,
            time_base: clip.time_base,
            hdr10: clip.hdr10,
        },
        codec,
    ))
}

impl VideoOut {
    /// Converts `decoded` to the encoder's format and sends it, stamped `pts`.
    fn send(&mut self, decoded: &frame::Video, pts: i64) -> Result<(), RecordError> {
        let (w, h) = (self.encoder.width(), self.encoder.height());
        let mut out = self
            .frame
            .take()
            .unwrap_or_else(|| frame::Video::new(self.pix, w, h));
        // SAFETY: a live frame of its own buffers. When the encoder still holds them (it keeps
        // frames to look ahead), they are copied away first: the encoder's stay untouched.
        unsafe {
            let code = ffi::av_frame_make_writable(out.as_mut_ptr());
            if code < 0 {
                return Err(ffmpeg_next::Error::from(code).into());
            }
            let o = out.as_mut_ptr();
            (*o).colorspace = (*decoded.as_ptr()).colorspace;
            (*o).color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
            if let Some(meta) = self.hdr10 {
                meta.attach_to_frame(o);
            }
        }
        let scaled = self.scaler.scale_into(&mut out, decoded);
        out.set_pts(Some(pts));
        let sent = scaled.and_then(|()| Ok(self.encoder.send_frame(&out)?));
        self.frame = Some(out);
        sent
    }
}

fn write_cut(
    clip: &Clip,
    (from, to): (usize, usize),
    tmp: &Path,
    dest: &Path,
    reencode: Option<&Reencode>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(), RecordError> {
    let muxer = match dest
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("mkv") => "matroska",
        Some("webm") => "webm",
        Some("mov") => "mov",
        _ => "mp4",
    };
    let mut input = format::input(&clip.path)?;
    let mut octx = format::output_as(tmp, muxer)?;
    let global = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let start_pts = clip.frames[from].pts;
    let end_pts = clip.frames[to].pts;
    let start_s = seconds(start_pts, clip.time_base);
    let end_s = seconds(end_pts, clip.time_base) + clip.frame_seconds();

    // The video: copied, or encoded again.
    let mut video_out = None;
    let mut map: Vec<Option<usize>> = vec![None; input.streams().count()];
    for s in input.streams() {
        let index = s.index();
        if index == clip.video {
            match reencode {
                Some(r) => {
                    let (out, codec) = open_encoder(clip, r, global)?;
                    let mut stream = octx.add_stream(codec)?;
                    stream.set_parameters(&out.encoder);
                    stream.set_time_base(out.time_base);
                    map[index] = Some(stream.index());
                    video_out = Some(out);
                }
                None => {
                    let mut stream = octx.add_stream(encoder::find(codec::Id::None))?;
                    stream.set_parameters(s.parameters());
                    // SAFETY: the parameters of the stream just created; a zero tag lets the
                    // muxer choose its own.
                    unsafe { (*stream.parameters().as_mut_ptr()).codec_tag = 0 };
                    stream.set_time_base(s.time_base());
                    map[index] = Some(stream.index());
                }
            }
        } else if s.parameters().medium() == media::Type::Audio {
            let mut stream = octx.add_stream(encoder::find(codec::Id::None))?;
            stream.set_parameters(s.parameters());
            // SAFETY: as above.
            unsafe { (*stream.parameters().as_mut_ptr()).codec_tag = 0 };
            stream.set_time_base(s.time_base());
            map[index] = Some(stream.index());
        }
    }
    octx.set_metadata(input.metadata().to_owned());
    let mut header = Dictionary::new();
    if muxer == "mp4" || muxer == "mov" {
        header.set("movflags", "faststart+use_metadata_tags");
    }
    octx.write_header_with(header)?;
    let out_tb: Vec<Rational> = octx.streams().map(|s| s.time_base()).collect();

    // The decoding order of the packets kept: from the key frame to the last one the part
    // needs.
    let first_decode = clip.frames[from..=to]
        .iter()
        .map(|f| f.decode)
        .min()
        .unwrap_or(0);
    let last_decode = clip.frames[from..=to]
        .iter()
        .map(|f| f.decode)
        .max()
        .unwrap_or(0);
    let key_pts = clip.frames[clip.key_before(from)].pts;
    let key_decode = clip.frames[clip.key_before(from)].decode;
    seek(&mut input, clip.video, key_pts)?;
    let mut decode_index = key_decode;
    let mut decoder = match video_out {
        // On the processor: the frames go to the encoder from memory.
        Some(_) => Some(
            video_decoder(
                input
                    .stream(clip.video)
                    .ok_or_else(|| RecordError::Config("no video".into()))?
                    .parameters(),
                false,
            )?
            .0,
        ),
        None => None,
    };
    let mut decoded = frame::Video::empty();
    let mut encoded = Packet::empty();
    let total = (to - from + 1) as f32;
    let mut done = 0_usize;
    let video_index = map[clip.video].unwrap_or(0);

    // One frame, in the encoder's time base: a packet without a duration would end the file
    // one frame early (the MP4 edit list leaves the last frame out).
    let frame_ticks = (i64::from(clip.time_base.1) * i64::from(clip.rate.1)
        / (i64::from(clip.time_base.0.max(1)) * i64::from(clip.rate.0.max(1))))
    .max(1);
    let mut write_encoded = |out: &mut VideoOut, octx: &mut format::context::Output| {
        while out.encoder.receive_packet(&mut encoded).is_ok() {
            if encoded.duration() <= 0 {
                encoded.set_duration(frame_ticks);
            }
            encoded.rescale_ts(out.time_base, out_tb[video_index]);
            encoded.set_stream(video_index);
            encoded.write_interleaved(octx)?;
        }
        Ok::<(), RecordError>(())
    };

    let mut video_done = false;
    for (s, mut packet) in input.packets() {
        if cancel.load(Ordering::Relaxed) {
            return Err(RecordError::Config("cancelled".into()));
        }
        let index = s.index();
        let Some(out_index) = map.get(index).copied().flatten() else {
            continue;
        };
        if index == clip.video {
            let this = decode_index;
            decode_index += 1;
            if video_done {
                continue;
            }
            match (&mut decoder, &mut video_out) {
                (Some(dec), Some(out)) => {
                    let _ = dec.send_packet(&packet);
                    while dec.receive_frame(&mut decoded).is_ok() {
                        let Some(pts) = decoded.timestamp().or(decoded.pts()) else {
                            continue;
                        };
                        if pts < start_pts || pts > end_pts {
                            continue;
                        }
                        out.send(&decoded, pts - start_pts)?;
                        write_encoded(out, &mut octx)?;
                        done += 1;
                        progress(done as f32 / total);
                    }
                    if this >= last_decode + 16 {
                        video_done = true;
                    }
                }
                _ => {
                    if this < first_decode || this > last_decode {
                        video_done = this > last_decode;
                        continue;
                    }
                    packet.set_pts(packet.pts().map(|p| p - start_pts));
                    packet.set_dts(packet.dts().map(|d| d - start_pts));
                    packet.rescale_ts(s.time_base(), out_tb[out_index]);
                    packet.set_position(-1);
                    packet.set_stream(out_index);
                    packet.write_interleaved(&mut octx)?;
                    done += 1;
                    progress(done as f32 / total);
                }
            }
        } else {
            // Audio: the packets that start within the part.
            let tb = s.time_base();
            let Some(pts) = packet.pts() else {
                continue;
            };
            let t = seconds(pts, tb);
            if t < start_s || t >= end_s {
                if t >= end_s && video_done {
                    break;
                }
                continue;
            }
            let shift = (start_s * f64::from(tb.1) / f64::from(tb.0.max(1))).round() as i64;
            packet.set_pts(Some(pts - shift));
            packet.set_dts(packet.dts().map(|d| d - shift));
            packet.rescale_ts(tb, out_tb[out_index]);
            packet.set_position(-1);
            packet.set_stream(out_index);
            packet.write_interleaved(&mut octx)?;
        }
    }
    // What the decoder and the encoder still hold.
    if let (Some(dec), Some(out)) = (&mut decoder, &mut video_out) {
        let _ = dec.send_eof();
        while dec.receive_frame(&mut decoded).is_ok() {
            if let Some(pts) = decoded.timestamp().or(decoded.pts())
                && (start_pts..=end_pts).contains(&pts)
            {
                out.send(&decoded, pts - start_pts)?;
                write_encoded(out, &mut octx)?;
            }
        }
        out.encoder.send_eof()?;
        write_encoded(out, &mut octx)?;
    }
    octx.write_trailer()?;
    progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PQ code (16 bits) of `nits`.
    fn pq(nits: f64) -> u16 {
        let (m1, m2) = (0.159_301_757_812_5, 78.843_75);
        let (c1, c2, c3) = (0.835_937_5, 18.851_562_5, 18.687_5);
        let y = (nits / 10_000.0).powf(m1);
        let e = ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2);
        (e * 65_535.0).round() as u16
    }

    #[test]
    fn hdr_is_tone_mapped_white_to_white_and_highlights_rolled_off() {
        // A clip made on a monitor whose SDR white is 240 nits, 1000 at its peak.
        let tone = ToneMap::new(240.0, 1000.0, 60.0);
        let h = 1000.0 / 240.0;
        let grey = |nits: f64| tone.pixel([pq(nits); 3], h)[0];
        assert_eq!(tone.pixel([0; 3], h), [0, 0, 0, 255]);
        // Dim light keeps its level.
        let expected = colour::srgb_encode(0.1) * 255.0;
        assert!((f32::from(grey(24.0)) - expected).abs() <= 2.0);
        // Brighter keeps getting brighter up to the peak, never clipped before it.
        let levels: Vec<u8> = [240.0, 400.0, 600.0, 900.0]
            .iter()
            .map(|&n| grey(n))
            .collect();
        assert!(levels.windows(2).all(|w| w[0] < w[1]), "{levels:?}");
        assert_eq!(grey(1000.0), 255);
        assert_eq!(grey(10_000.0), 255);
        // Hue kept: a bright red stays red, not white.
        let red = tone.pixel([pq(2000.0), pq(5.0), pq(5.0)], h);
        assert!(red[2] == 255 && red[1] < 120 && red[0] < 120, "{red:?}");
    }

    #[test]
    fn a_frame_without_highlights_keeps_the_clip_s_sdr_white() {
        let tone = ToneMap::new(240.0, 1000.0, 60.0);
        // Two pixels of 16-bit PQ: SDR white and 80 % of it.
        let frame = |nits: [f64; 2]| -> Vec<u8> {
            nits.iter()
                .flat_map(|&n| [pq(n); 3])
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        let mut out = vec![0; 8];
        tone.run(&frame([240.0, 192.0]), 12, 2, 1, &mut out);
        assert_eq!(&out[..3], &[255; 3]);
        let light_grey = (colour::srgb_encode(0.8) * 255.0).round();
        assert!((f32::from(out[4]) - light_grey).abs() <= 1.0, "{}", out[4]);
        // A highlight comes: followed quickly, white is rolled off; after a seek, at once.
        tone.reset();
        tone.run(&frame([240.0, 1000.0]), 12, 2, 1, &mut out);
        assert!(out[0] < 250, "{}", out[0]);
        assert_eq!(&out[4..7], &[255; 3]);
    }

    fn clip(frames: &[(i64, bool, usize)]) -> Clip {
        Clip {
            path: PathBuf::new(),
            width: 1920,
            height: 1080,
            rate: Rational(60, 1),
            time_base: Rational(1, 60),
            frames: frames
                .iter()
                .map(|&(pts, key, decode)| Frame { pts, key, decode })
                .collect(),
            audio_tracks: 0,
            hdr: false,
            deep: false,
            hdr10: None,
            sdr_white_nits: REFERENCE_WHITE,
            video: 0,
        }
    }

    #[test]
    fn a_part_from_a_key_frame_is_copied_as_it_is() {
        // Two GOPs of three frames, no reordering.
        let c = clip(&[
            (0, true, 0),
            (1, false, 1),
            (2, false, 2),
            (3, true, 3),
            (4, false, 4),
            (5, false, 5),
        ]);
        assert!(c.lossless(0, 5));
        assert!(c.lossless(3, 4));
        assert!(!c.lossless(1, 4), "starts between two key frames");
        assert_eq!(c.key_before(5), 3);
        assert_eq!(c.key_before(2), 0);
    }

    #[test]
    fn a_part_that_ends_before_a_reordered_frame_is_encoded_again() {
        // I P B B: the P frame (shown 3rd) is stored before the two B frames.
        let c = clip(&[(0, true, 0), (1, false, 2), (2, false, 3), (3, false, 1)]);
        assert!(c.lossless(0, 3));
        // Frames 0..=1 need packet 2, which comes after packet 1 (shown last).
        assert!(!c.lossless(0, 1));
    }
}
