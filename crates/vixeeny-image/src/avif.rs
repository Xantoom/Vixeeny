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
    for (y, out) in source
        .data_mut(0)
        .chunks_mut(stride)
        .take(image.height as usize)
        .enumerate()
    {
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
        for (x, (o, px)) in out
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(row.as_chunks::<3>().0)
            .enumerate()
        {
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
    write(yuv, colours, &settings)
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
    match av1 {
        // All-intra, the tune for still pictures (libaom 3.12 and later). Its speeds are slower
        // than SVT-AV1's: two steps up keep a 4K capture near a second at the default speed (6).
        Av1::Aom => {
            options.set("usage", "allintra");
            options.set("still-picture", "1");
            options.set("cpu-used", &(settings.speed + 2).min(8).to_string());
            options.set("aom-params", "tune=iq");
        }
        // Still-picture mode and the tune for still pictures (IQ).
        Av1::Svt => {
            options.set("preset", &settings.speed.min(10).to_string());
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
