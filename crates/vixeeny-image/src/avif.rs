// SPDX-License-Identifier: GPL-3.0-or-later
//! AVIF through the FFmpeg Vixeeny ships: libaom encodes (all-intra, tuned for still pictures, as
//! libavif sets it; SVT-AV1 when this FFmpeg has no libaom), the `avif` muxer writes the file,
//! dav1d decodes. The colours are described by the file's `nclx` box: sRGB, BT.601 matrix, full
//! range.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ffmpeg_next::format::Pixel;
use ffmpeg_next::util::color::{Primaries, Range, Space, TransferCharacteristic};
use ffmpeg_next::{Packet, Rational, codec, decoder, encoder, ffi, format, frame, media};
use vixeeny_common::config::AVIF_MAX_SPEED;

use crate::hdr::Light;
use crate::{AvifSettings, Bgra, Chroma, Hdr, ImageError};

fn err(what: &str) -> impl FnOnce(ffmpeg_next::Error) -> ImageError + '_ {
    move |e| ImageError::Avif(format!("{what}: {e}"))
}

/// FFmpeg reads and writes files: a private one in the temporary folder, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("vixeeny-{}-{n}.avif", std::process::id())))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The AV1 encoder: libaom, else SVT-AV1 (which only encodes 4:2:0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av1 {
    Aom,
    Svt,
}

impl Av1 {
    fn find() -> Result<(Self, ffmpeg_next::Codec), ImageError> {
        encoder::find_by_name("libaom-av1")
            .map(|c| (Self::Aom, c))
            .or_else(|| encoder::find_by_name("libsvtav1").map(|c| (Self::Svt, c)))
            .ok_or_else(|| ImageError::Avif("no AV1 encoder in this FFmpeg".into()))
    }
}

pub(crate) fn encode(image: &Bgra<'_>, settings: &AvifSettings) -> Result<Vec<u8>, ImageError> {
    ffmpeg_next::init().map_err(err("init"))?;
    let full = settings.chroma == Chroma::Yuv444 && Av1::find()?.0 == Av1::Aom;
    let pixel = match (settings.depth, full) {
        (8, false) => Pixel::YUV420P,
        (10, false) => Pixel::YUV420P10LE,
        (8, true) => Pixel::YUV444P,
        (10, true) => Pixel::YUV444P10LE,
        (other, _) => return Err(ImageError::Avif(format!("unsupported depth {other}"))),
    };
    let mut source = frame::Video::new(Pixel::BGRA, image.width, image.height);
    let stride = source.stride(0);
    let row = image.width as usize * 4;
    let rows = source
        .data_mut(0)
        .chunks_mut(stride)
        .take(image.height as usize)
        .enumerate();
    for (y, out) in rows {
        out[..row].copy_from_slice(&image.data[y * image.stride..y * image.stride + row]);
    }
    source.set_color_space(Space::RGB);
    source.set_color_range(Range::JPEG);
    let mut yuv = frame::Video::new(pixel, image.width, image.height);
    yuv.set_color_space(Space::BT470BG);
    yuv.set_color_range(Range::JPEG);
    scale(&source, &mut yuv)?;
    let colours = (
        Space::BT470BG,
        Primaries::BT709,
        TransferCharacteristic::IEC61966_2_1,
    );
    write(yuv, colours, settings)
}

/// 10-bit 4:2:0, BT.2020 / PQ, full range. The conversion to YCbCr is done here rather than by
/// swscale, whose matrix choice follows the frame tags: BT.2020 non-constant luminance.
pub(crate) fn encode_hdr(image: &Hdr<'_>, settings: &AvifSettings) -> Result<Vec<u8>, ImageError> {
    ffmpeg_next::init().map_err(err("init"))?;
    let (w, h) = (image.width as usize, image.height as usize);
    let pq = image.to_pq();
    let mut yuv = frame::Video::new(Pixel::YUV420P10LE, image.width, image.height);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut cb = vec![0.0_f32; cw * ch];
    let mut cr = vec![0.0_f32; cw * ch];
    let y_stride = yuv.stride(0);
    let luma = yuv.data_mut(0);
    for (y, row) in pq.chunks_exact(w * 3).enumerate() {
        let out = &mut luma[y * y_stride..y * y_stride + w * 2];
        let pixels = out
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(row.as_chunks::<3>().0)
            .enumerate();
        for (x, (o, px)) in pixels {
            let [r, g, b] = px.map(|c| f32::from(c) / 65535.0);
            let l = 0.2627 * r + 0.6780 * g + 0.0593 * b;
            *o = code(l).to_le_bytes();
            let i = (y / 2) * cw + x / 2;
            cb[i] += (b - l) / 1.8814;
            cr[i] += (r - l) / 1.4746;
        }
    }
    // Each chroma sample is the mean of the (up to four) pixels it covers.
    for (plane, sums) in [(1, &cb), (2, &cr)] {
        let stride = yuv.stride(plane);
        let data = yuv.data_mut(plane);
        for cy in 0..ch {
            let rows = (h - cy * 2).min(2);
            for cx in 0..cw {
                let n = (rows * (w - cx * 2).min(2)) as f32;
                let v = code(sums[cy * cw + cx] / n + 0.5);
                let o = cy * stride + cx * 2;
                data[o..o + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    yuv.set_color_space(Space::BT2020NCL);
    yuv.set_color_range(Range::JPEG);
    let colours = (
        Space::BT2020NCL,
        Primaries::BT2020,
        TransferCharacteristic::SMPTE2084,
    );
    let settings = AvifSettings {
        depth: 10,
        ..*settings
    };
    let avif = write(yuv, colours, &settings)?;
    // Without them the file is still a correct HDR picture, tone mapped from 10 000 nits.
    Ok(with_light(&avif, &image.light()).unwrap_or(avif))
}

/// 0…1 → a full-range 10-bit code.
fn code(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 1023.0).round() as u16
}

/// Encodes one picture and wraps it in an AVIF file.
fn write(
    mut yuv: frame::Video,
    (space, primaries, transfer): (Space, Primaries, TransferCharacteristic),
    settings: &AvifSettings,
) -> Result<Vec<u8>, ImageError> {
    let (av1, codec) = Av1::find()?;
    yuv.set_pts(Some(0));
    let scratch = Scratch::new();
    let mut octx = format::output_as(scratch.path(), "avif").map_err(err("muxer"))?;
    let mut ctx = codec::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(err("encoder"))?;
    ctx.set_width(yuv.width());
    ctx.set_height(yuv.height());
    ctx.set_format(yuv.format());
    ctx.set_time_base(Rational(1, 1));
    ctx.set_color_range(Range::JPEG);
    ctx.set_colorspace(space);
    ctx.set_color_primaries(primaries);
    ctx.set_color_transfer_characteristic(transfer);
    if octx.format().flags().contains(format::Flags::GLOBAL_HEADER) {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    // libavif's mapping: quality 0..=100 to a quantizer 63..=0 (0 would be lossless).
    let quality = i32::from(settings.quality.min(100));
    let quantizer = (((100 - quality) * 63 + 50) / 100).max(1);
    let mut options = ffmpeg_next::Dictionary::new();
    options.set("crf", &quantizer.to_string());
    // Every core: FFmpeg's encoders take one unless told (libaom three times faster at 4K, the
    // same file).
    options.set("threads", "0");
    match av1 {
        // All-intra, the tune for still pictures (libaom 3.12 and later). Its speeds are slower
        // than SVT-AV1's: two steps up (2 to 8, FFmpeg's last) keep a 4K capture near a third of
        // a second at the fastest.
        Av1::Aom => {
            options.set("usage", "allintra");
            options.set("still-picture", "1");
            options.set(
                "cpu-used",
                &(settings.speed.min(AVIF_MAX_SPEED) + 2).to_string(),
            );
            options.set("aom-params", "tune=iq");
        }
        // Still-picture mode and the tune for still pictures (IQ).
        Av1::Svt => {
            options.set(
                "preset",
                &(settings.speed.min(AVIF_MAX_SPEED) + 4).to_string(),
            );
            options.set("svtav1-params", "avif=1:tune=3");
        }
    }
    let mut encoder = ctx.open_with(options).map_err(err("encoder"))?;
    let mut stream = octx.add_stream(codec).map_err(err("stream"))?;
    stream.set_parameters(&encoder);
    octx.write_header().map_err(err("header"))?;
    let time_base = octx.stream(0).map_or(Rational(1, 1), |s| s.time_base());
    encoder.send_frame(&yuv).map_err(err("encode"))?;
    encoder.send_eof().map_err(err("encode"))?;
    let mut packet = Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(0);
        packet.rescale_ts(Rational(1, 1), time_base);
        packet.write_interleaved(&mut octx).map_err(err("write"))?;
    }
    octx.write_trailer().map_err(err("trailer"))?;
    drop(octx);
    std::fs::read(scratch.path()).map_err(|e| ImageError::Avif(format!("read back: {e}")))
}

/// A box of an ISO BMFF file: its type and where it starts, its contents start and it ends.
struct Bmff {
    kind: [u8; 4],
    start: usize,
    body: usize,
    end: usize,
}

/// The boxes laid one after another in `data[range]`; `None` if they do not fill it exactly
/// (or use the 64-bit and to-the-end sizes, which FFmpeg's AVIF never does).
fn boxes(data: &[u8], range: std::ops::Range<usize>) -> Option<Vec<Bmff>> {
    let mut out = Vec::new();
    let mut at = range.start;
    while at < range.end {
        let size = u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
        let kind = data.get(at + 4..at + 8)?.try_into().ok()?;
        if size < 8 || at + size > range.end {
            return None;
        }
        out.push(Bmff {
            kind,
            start: at,
            body: at + 8,
            end: at + size,
        });
        at += size;
    }
    Some(out)
}

fn find<'a>(boxes: &'a [Bmff], kind: &[u8; 4]) -> Option<&'a Bmff> {
    boxes.iter().find(|b| &b.kind == kind)
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// The AVIF FFmpeg wrote with `clli` and `mdcv` properties on its picture (its muxer writes them
/// for MP4 tracks only), so viewers tone map from the picture's light and screen rather than
/// 10 000 nits; `None` when the file is not laid out as that muxer lays it.
fn with_light(avif: &[u8], light: &Light) -> Option<Vec<u8>> {
    let top = boxes(avif, 0..avif.len())?;
    let meta = find(&top, b"meta")?;
    // A full box: version and flags first.
    let in_meta = boxes(avif, meta.body + 4..meta.end)?;
    let iloc = find(&in_meta, b"iloc")?;
    let iprp = find(&in_meta, b"iprp")?;
    let primary = match find(&in_meta, b"pitm") {
        Some(pitm) => u16_at(avif, pitm.body + 4)?,
        None => 1,
    };
    let in_iprp = boxes(avif, iprp.body..iprp.end)?;
    let ipco = find(&in_iprp, b"ipco")?;
    let ipma = find(&in_iprp, b"ipma")?;
    let first = boxes(avif, ipco.body..ipco.end)?.len() + 1;

    // Associations as single bytes: version 0, flags without bit 0.
    if avif.get(ipma.body..ipma.body + 4)? != [0, 0, 0, 0] || first + 1 > 0x7f {
        return None;
    }
    let entries = u32::from_be_bytes(avif.get(ipma.body + 4..ipma.body + 8)?.try_into().ok()?);
    let mut at = ipma.body + 8;
    let mut count_at = None;
    for _ in 0..entries {
        let (item, count) = (u16_at(avif, at)?, *avif.get(at + 2)?);
        if item == primary {
            count_at = Some(at + 2);
        }
        at += 3 + usize::from(count);
    }
    let count_at = count_at?;
    let associated = count_at + 1 + usize::from(avif[count_at]);
    if avif[count_at] > 0xff - 2 || at != ipma.end {
        return None;
    }

    let mut properties = Vec::with_capacity(44);
    properties.extend_from_slice(&12u32.to_be_bytes());
    properties.extend_from_slice(b"clli");
    for nits in [light.max_cll.ceil(), light.max_fall.round()] {
        properties.extend_from_slice(&(nits.min(65_535.0) as u16).to_be_bytes());
    }
    properties.extend_from_slice(&32u32.to_be_bytes());
    properties.extend_from_slice(b"mdcv");
    // Green, blue, red, as HEVC's SEI orders them, then the white; x, y in 0.00002.
    let [red, green, blue] = Light::PRIMARIES;
    for v in [green, blue, red, Light::WHITE].iter().flatten() {
        properties.extend_from_slice(&((v * 50_000.0).round() as u16).to_be_bytes());
    }
    // The brightest and the darkest light, in 0.0001 nits.
    for nits in [f64::from(light.peak), Light::BLACK_NITS] {
        properties.extend_from_slice(&((nits * 10_000.0).round() as u32).to_be_bytes());
    }
    let associations = [first as u8, first as u8 + 1];

    // Where the bytes go, in the order of the file; a byte of the original moves by all that is
    // put at or before it.
    let mut inserts = [
        (ipco.end, properties.as_slice()),
        (associated, associations.as_slice()),
    ];
    inserts.sort_by_key(|i| i.0);
    let moved = |p: usize| {
        p + inserts
            .iter()
            .filter(|i| i.0 <= p)
            .map(|i| i.1.len())
            .sum::<usize>()
    };
    let mut out = Vec::with_capacity(avif.len() + 46);
    let mut from = 0;
    for (at, bytes) in inserts {
        out.extend_from_slice(&avif[from..at]);
        out.extend_from_slice(bytes);
        from = at;
    }
    out.extend_from_slice(&avif[from..]);

    let grow = |out: &mut [u8], at: usize, by: usize| -> Option<()> {
        let field = out.get_mut(at..at + 4)?;
        let size = u32::from_be_bytes((&*field).try_into().ok()?) as usize + by;
        field.copy_from_slice(&u32::try_from(size).ok()?.to_be_bytes());
        Some(())
    };
    let added = properties.len() + associations.len();
    grow(&mut out, moved(meta.start), added)?;
    grow(&mut out, moved(iprp.start), added)?;
    grow(&mut out, moved(ipco.start), properties.len())?;
    grow(&mut out, moved(ipma.start), associations.len())?;
    out[moved(count_at)] += 2;

    // The pictures' places in the file: version 0, 32-bit offsets and lengths, no base offset.
    if avif.get(iloc.body..iloc.body + 6)? != [0, 0, 0, 0, 0x44, 0] {
        return None;
    }
    let items = u16_at(avif, iloc.body + 6)?;
    let mut at = iloc.body + 8;
    for _ in 0..items {
        let extents = u16_at(avif, at + 4)?;
        at += 6;
        for _ in 0..extents {
            let offset = u32::from_be_bytes(avif.get(at..at + 4)?.try_into().ok()?) as usize;
            let field = moved(at);
            out[field..field + 4]
                .copy_from_slice(&u32::try_from(moved(offset)).ok()?.to_be_bytes());
            at += 8;
        }
    }
    (at == iloc.end).then_some(out)
}

/// What dav1d decoded: tightly packed RGBA8.
pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Decoded, ImageError> {
    ffmpeg_next::init().map_err(err("init"))?;
    let scratch = Scratch::new();
    std::fs::write(scratch.path(), bytes).map_err(|e| ImageError::Avif(e.to_string()))?;
    let mut input = format::input(scratch.path()).map_err(err("open"))?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| ImageError::Avif("no picture".into()))?;
    let index = stream.index();
    let ctx = codec::Context::from_parameters(stream.parameters()).map_err(err("decoder"))?;
    // dav1d, never the AV1 decoder of FFmpeg itself, which needs the graphics card.
    let dav1d = decoder::find_by_name("libdav1d")
        .ok_or_else(|| ImageError::Avif("no dav1d in this FFmpeg".into()))?;
    let mut decoder = ctx
        .decoder()
        .open_as(dav1d)
        .and_then(|o| o.video())
        .map_err(err("decoder"))?;
    let mut picture = frame::Video::empty();
    let mut got = false;
    for (s, packet) in input.packets() {
        if s.index() == index && decoder.send_packet(&packet).is_ok() {
            got = decoder.receive_frame(&mut picture).is_ok();
            if got {
                break;
            }
        }
    }
    if !got {
        decoder.send_eof().map_err(err("decode"))?;
        decoder.receive_frame(&mut picture).map_err(err("decode"))?;
    }
    let (width, height) = (picture.width(), picture.height());
    if picture.color_transfer_characteristic() == TransferCharacteristic::SMPTE2084 {
        return decode_hdr(&picture);
    }
    let mut rgba = frame::Video::new(Pixel::RGBA, width, height);
    rgba.set_color_space(Space::RGB);
    rgba.set_color_range(Range::JPEG);
    scale(&picture, &mut rgba)?;
    let stride = rgba.stride(0);
    let row = width as usize * 4;
    let mut out = Vec::with_capacity(row * height as usize);
    for line in rgba.data(0).chunks(stride).take(height as usize) {
        out.extend_from_slice(&line[..row]);
    }
    Ok(Decoded {
        width,
        height,
        rgba: out,
    })
}

/// An HDR (PQ) picture, tone-mapped to SDR.
fn decode_hdr(picture: &frame::Video) -> Result<Decoded, ImageError> {
    let (width, height) = (picture.width(), picture.height());
    let mut rgb = frame::Video::new(Pixel::RGB48LE, width, height);
    rgb.set_color_space(Space::RGB);
    rgb.set_color_range(Range::JPEG);
    scale(picture, &mut rgb)?;
    let stride = rgb.stride(0);
    let row = width as usize * 6;
    let signal: Vec<[f32; 3]> = rgb
        .data(0)
        .chunks(stride)
        .take(height as usize)
        .flat_map(|line| line[..row].as_chunks::<6>().0)
        .map(|px| {
            let c = |i: usize| f32::from(u16::from_le_bytes([px[i], px[i + 1]])) / 65535.0;
            [c(0), c(2), c(4)]
        })
        .collect();
    let mut rgba = crate::hdr::pq_to_sdr(width, height, &signal);
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Ok(Decoded {
        width,
        height,
        rgba,
    })
}

/// `src` into `dst` (allocated), with the matrix and range each frame declares. Only those
/// change: `dst` takes the primaries and transfer of `src`, or swscale would convert them too.
pub(crate) fn scale(src: &frame::Video, dst: &mut frame::Video) -> Result<(), ImageError> {
    // SAFETY: a fresh context, freed below on every path; both frames are live and allocated,
    // and swscale only reads `src`.
    unsafe {
        let (from, to) = (src.as_ptr(), dst.as_mut_ptr());
        (*to).color_primaries = (*from).color_primaries;
        (*to).color_trc = (*from).color_trc;
        let mut ctx = ffi::sws_alloc_context();
        if ctx.is_null() {
            return Err(ImageError::Avif("cannot allocate a scaler".into()));
        }
        let flags = ffi::SwsFlags::SWS_BICUBIC as u32
            | ffi::SwsFlags::SWS_ACCURATE_RND as u32
            | ffi::SwsFlags::SWS_FULL_CHR_H_INT as u32;
        ffi::av_opt_set_int(ctx.cast(), c"sws_flags".as_ptr(), i64::from(flags), 0);
        ffi::av_opt_set_int(ctx.cast(), c"threads".as_ptr(), 0, 0);
        let code = ffi::sws_scale_frame(ctx, to, from);
        ffi::sws_free_context(&raw mut ctx);
        if code < 0 {
            return Err(err("convert")(ffmpeg_next::Error::from(code)));
        }
    }
    Ok(())
}

/// Test helper: tightly packed RGB8.
#[cfg(test)]
pub(crate) fn decode_rgb(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let d = decode(bytes)?;
    let rgb = d
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    Ok((d.width, d.height, rgb))
}
