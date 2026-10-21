// SPDX-License-Identifier: GPL-3.0-or-later
//! Animated pictures from a run of frames (a part of a clip): GIF (each frame reduced to 256
//! colours by libimagequant, dithered), animated WebP (libwebp) and animated AVIF (SVT-AV1 and
//! the `avif` muxer of FFmpeg). Frames are handed one at a time, in order, with their time; the
//! file is written by [`Animation::finish`].

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use ffmpeg_next::format::Pixel;
use ffmpeg_next::util::color::{Primaries, Range, Space, TransferCharacteristic};
use ffmpeg_next::{Packet, Rational, codec, encoder, format, frame};

use crate::{ImageError, Settings};

/// The animated formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimFormat {
    Gif,
    WebP,
    Avif,
}

impl AnimFormat {
    pub const ALL: [Self; 3] = [Self::Gif, Self::WebP, Self::Avif];

    /// From a file extension, any case.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::WebP),
            "avif" => Some(Self::Avif),
            _ => None,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::WebP => "webp",
            Self::Avif => "avif",
        }
    }

    /// Frames per second at most: GIF times are in hundredths of a second and its frames are
    /// heavy, the others keep the motion smooth.
    pub const fn max_fps(self) -> f64 {
        match self {
            Self::Gif => 15.0,
            Self::WebP | Self::Avif => 30.0,
        }
    }

    /// The longer side at most, in pixels: what is shared in a message or a ticket.
    pub const fn max_side(self) -> u32 {
        match self {
            Self::Gif => 640,
            Self::WebP | Self::Avif => 1280,
        }
    }
}

/// The size an animation of a `width`×`height` video gets: within `max_side`, never enlarged,
/// even on both sides (4:2:0 for AVIF), the shape kept.
pub fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let scale = (f64::from(max_side) / f64::from(width.max(height).max(1))).min(1.0);
    let even = |v: u32| ((f64::from(v) * scale).round() as u32 & !1).max(2);
    (even(width), even(height))
}

/// Which of the frames shown at `times` (seconds, rising) an animation at `fps` keeps: the
/// first of each `1 / fps` slot.
pub fn pick(times: &[f64], fps: f64) -> Vec<usize> {
    let Some(&start) = times.first() else {
        return Vec::new();
    };
    let mut kept = Vec::new();
    let mut next_slot = 0_i64;
    for (i, &t) in times.iter().enumerate() {
        let slot = ((t - start) * fps + 1e-6).floor() as i64;
        if slot >= next_slot {
            kept.push(i);
            next_slot = slot + 1;
        }
    }
    kept
}

/// The delays of GIF frames starting at `starts_ms` and ending at `end_ms`, in hundredths of a
/// second; rounded on the running total so that the animation lasts as long as the clip.
fn gif_delays(starts_ms: &[u64], end_ms: u64) -> Vec<u16> {
    let cs = |ms: u64| (ms + 5) / 10;
    starts_ms
        .iter()
        .zip(starts_ms.iter().skip(1).chain(std::iter::once(&end_ms)))
        .map(|(&a, &b)| (cs(b).saturating_sub(cs(a))).clamp(2, u64::from(u16::MAX)) as u16)
        .collect()
}

enum Inner {
    Gif {
        encoder: gif::Encoder<BufWriter<File>>,
        /// Frames wait for the next one's time to know how long they last.
        pending: Option<(Vec<u8>, Vec<u8>, u64)>,
        quality: u8,
    },
    WebP {
        encoder: *mut libwebp_sys::WebPAnimEncoder,
        config: libwebp_sys::WebPConfig,
    },
    Avif {
        octx: format::context::Output,
        encoder: encoder::Video,
        time_base: Rational,
    },
}

/// An animation being written to a file.
pub struct Animation {
    inner: Inner,
    dest: PathBuf,
    width: u32,
    height: u32,
}

// SAFETY: the WebP encoder is used through `&mut self` only, by one thread at a time.
unsafe impl Send for Animation {}

impl Animation {
    /// An animation of `width`×`height` frames written to `dest`, with the quality of the
    /// image settings (WebP, AVIF; GIF has none of its own and takes WebP's).
    pub fn new(
        format: AnimFormat,
        width: u32,
        height: u32,
        settings: &Settings,
        dest: &Path,
    ) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::BadBuffer("empty animation"));
        }
        let quality = settings.webp.quality.clamp(0.0, 100.0);
        let inner = match format {
            AnimFormat::Gif => {
                if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
                    return Err(ImageError::Gif("too large".into()));
                }
                let err = |e: std::io::Error| ImageError::Gif(e.to_string());
                let file = BufWriter::new(File::create(dest).map_err(err)?);
                let mut encoder = gif::Encoder::new(file, width as u16, height as u16, &[])
                    .map_err(|e| ImageError::Gif(e.to_string()))?;
                encoder
                    .set_repeat(gif::Repeat::Infinite)
                    .map_err(|e| ImageError::Gif(e.to_string()))?;
                Inner::Gif {
                    encoder,
                    pending: None,
                    quality: quality as u8,
                }
            }
            AnimFormat::WebP => webp_encoder(width, height, quality)?,
            AnimFormat::Avif => avif_encoder(width, height, settings, dest)?,
        };
        Ok(Self {
            inner,
            dest: dest.to_owned(),
            width,
            height,
        })
    }

    /// Adds a frame (top-down BGRA, tightly packed, the size given to [`Animation::new`]),
    /// shown from `ms` milliseconds after the start.
    pub fn push(&mut self, bgra: &[u8], ms: u64) -> Result<(), ImageError> {
        let (w, h) = (self.width, self.height);
        if bgra.len() < w as usize * h as usize * 4 {
            return Err(ImageError::BadBuffer("frame smaller than the animation"));
        }
        match &mut self.inner {
            Inner::Gif {
                encoder,
                pending,
                quality,
            } => {
                let (palette, indices) = quantize(w, h, bgra, *quality)?;
                if let Some((palette, indices, start)) = pending.replace((palette, indices, ms)) {
                    write_gif_frame(encoder, (w, h), &palette, indices, start, ms)?;
                }
                Ok(())
            }
            Inner::WebP { encoder, config } => {
                let err = |what: &str| ImageError::WebP(what.to_owned());
                let mut picture = libwebp_sys::WebPPicture::new().map_err(|()| err("picture"))?;
                picture.use_argb = 1;
                picture.width = w as i32;
                picture.height = h as i32;
                // SAFETY: `bgra` holds w × h tightly packed pixels; libwebp copies them. The
                // picture is freed on every path, after the encoder took what it needs.
                unsafe {
                    let imported = libwebp_sys::WebPPictureImportBGRA(
                        &raw mut picture,
                        bgra.as_ptr(),
                        w as i32 * 4,
                    );
                    let added = imported != 0
                        && libwebp_sys::WebPAnimEncoderAdd(
                            *encoder,
                            &raw mut picture,
                            ms as i32,
                            &raw const *config,
                        ) != 0;
                    libwebp_sys::WebPPictureFree(&raw mut picture);
                    if !added {
                        return Err(webp_error(*encoder));
                    }
                }
                Ok(())
            }
            Inner::Avif {
                octx,
                encoder,
                time_base,
            } => {
                let mut source = frame::Video::new(Pixel::BGRA, w, h);
                let stride = source.stride(0);
                let row = w as usize * 4;
                for (y, out) in source
                    .data_mut(0)
                    .chunks_mut(stride)
                    .take(h as usize)
                    .enumerate()
                {
                    out[..row].copy_from_slice(&bgra[y * row..(y + 1) * row]);
                }
                source.set_color_space(Space::RGB);
                source.set_color_range(Range::JPEG);
                let mut yuv = frame::Video::new(Pixel::YUV420P, w, h);
                yuv.set_color_space(Space::BT470BG);
                yuv.set_color_range(Range::JPEG);
                crate::avif::scale(&source, &mut yuv)?;
                yuv.set_pts(Some(ms as i64));
                encoder.send_frame(&yuv).map_err(avif_err("encode"))?;
                drain(encoder, octx, *time_base)
            }
        }
    }

    /// Writes the end of the file; the animation lasts until `end_ms`.
    pub fn finish(mut self, end_ms: u64) -> Result<(), ImageError> {
        let (w, h) = (self.width, self.height);
        match &mut self.inner {
            Inner::Gif {
                encoder, pending, ..
            } => {
                if let Some((palette, indices, start)) = pending.take() {
                    write_gif_frame(encoder, (w, h), &palette, indices, start, end_ms)?;
                }
                Ok(())
            }
            Inner::WebP { encoder, .. } => {
                let mut data = libwebp_sys::WebPData {
                    bytes: std::ptr::null(),
                    size: 0,
                };
                // SAFETY: a NULL frame ends the animation at `end_ms`; the assembled data is
                // copied out, then freed.
                unsafe {
                    let ended = libwebp_sys::WebPAnimEncoderAdd(
                        *encoder,
                        std::ptr::null_mut(),
                        end_ms as i32,
                        std::ptr::null(),
                    ) != 0;
                    if !ended || libwebp_sys::WebPAnimEncoderAssemble(*encoder, &raw mut data) == 0
                    {
                        return Err(webp_error(*encoder));
                    }
                    let bytes = std::slice::from_raw_parts(data.bytes, data.size).to_vec();
                    libwebp_sys::WebPDataClear(&mut data);
                    std::fs::write(&self.dest, bytes).map_err(|e| ImageError::WebP(e.to_string()))
                }
            }
            Inner::Avif {
                octx,
                encoder,
                time_base,
            } => {
                encoder.send_eof().map_err(avif_err("encode"))?;
                drain(encoder, octx, *time_base)?;
                octx.write_trailer().map_err(avif_err("trailer"))
            }
        }
    }
}

impl Drop for Animation {
    fn drop(&mut self) {
        if let Inner::WebP { encoder, .. } = &self.inner {
            // SAFETY: created by `WebPAnimEncoderNewInternal`, deleted once.
            unsafe { libwebp_sys::WebPAnimEncoderDelete(*encoder) };
        }
    }
}

/// 256 colours at most for the frame, dithered: the palette (RGB) and one index per pixel.
fn quantize(w: u32, h: u32, bgra: &[u8], quality: u8) -> Result<(Vec<u8>, Vec<u8>), ImageError> {
    let err = |e: imagequant::Error| ImageError::Gif(format!("colours: {e}"));
    let pixels: Vec<imagequant::RGBA> = bgra[..w as usize * h as usize * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[b, g, r, _]| imagequant::RGBA::new(r, g, b, 255))
        .collect();
    let mut attributes = imagequant::new();
    attributes.set_speed(5).map_err(err)?;
    attributes.set_quality(0, quality.min(100)).map_err(err)?;
    let mut image = attributes
        .new_image(pixels, w as usize, h as usize, 0.0)
        .map_err(err)?;
    let mut result = attributes.quantize(&mut image).map_err(err)?;
    result.set_dithering_level(1.0).map_err(err)?;
    let (palette, indices) = result.remapped(&mut image).map_err(err)?;
    let palette = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
    Ok((palette, indices))
}

fn write_gif_frame(
    encoder: &mut gif::Encoder<BufWriter<File>>,
    (w, h): (u32, u32),
    palette: &[u8],
    indices: Vec<u8>,
    start_ms: u64,
    end_ms: u64,
) -> Result<(), ImageError> {
    let delay = gif_delays(&[start_ms], end_ms)[0];
    let frame = gif::Frame {
        width: w as u16,
        height: h as u16,
        delay,
        palette: Some(palette.to_vec()),
        buffer: std::borrow::Cow::Owned(indices),
        ..gif::Frame::default()
    };
    encoder
        .write_frame(&frame)
        .map_err(|e| ImageError::Gif(e.to_string()))
}

fn webp_encoder(width: u32, height: u32, quality: f32) -> Result<Inner, ImageError> {
    let err = |what: &str| ImageError::WebP(what.to_owned());
    if width > 16383 || height > 16383 {
        return Err(err("WebP is limited to 16383 pixels per side"));
    }
    let mut config = libwebp_sys::WebPConfig::new().map_err(|()| err("config init"))?;
    config.quality = quality;
    config.method = 4;
    // SAFETY: `options` is filled in by the init call before use; the encoder copies it.
    unsafe {
        let mut options = std::mem::MaybeUninit::<libwebp_sys::WebPAnimEncoderOptions>::uninit();
        let abi = libwebp_sys::WEBP_MUX_ABI_VERSION as i32;
        if libwebp_sys::WebPAnimEncoderOptionsInitInternal(options.as_mut_ptr(), abi) == 0 {
            return Err(err("options init"));
        }
        let mut options = options.assume_init();
        // Mixing lossy and lossless frames keeps flat screen content sharp.
        options.allow_mixed = 1;
        let encoder = libwebp_sys::WebPAnimEncoderNewInternal(
            width as i32,
            height as i32,
            &raw const options,
            abi,
        );
        if encoder.is_null() {
            return Err(err("encoder init"));
        }
        Ok(Inner::WebP { encoder, config })
    }
}

/// The encoder's own message.
///
/// # Safety
/// `encoder` is a live encoder.
unsafe fn webp_error(encoder: *mut libwebp_sys::WebPAnimEncoder) -> ImageError {
    // SAFETY: the message is a NUL-terminated string owned by the live encoder.
    let message = unsafe {
        let text = libwebp_sys::WebPAnimEncoderGetError(encoder);
        if text.is_null() {
            "encoder error".to_owned()
        } else {
            std::ffi::CStr::from_ptr(text)
                .to_string_lossy()
                .into_owned()
        }
    };
    ImageError::WebP(message)
}

fn avif_err(what: &str) -> impl FnOnce(ffmpeg_next::Error) -> ImageError + '_ {
    move |e| ImageError::Avif(format!("{what}: {e}"))
}

/// SVT-AV1 at the AVIF quality of the settings, a faster preset than a still picture (there
/// are many frames), timed in milliseconds.
fn avif_encoder(
    width: u32,
    height: u32,
    settings: &Settings,
    dest: &Path,
) -> Result<Inner, ImageError> {
    ffmpeg_next::init().map_err(avif_err("init"))?;
    let codec = encoder::find_by_name("libsvtav1")
        .ok_or_else(|| ImageError::Avif("no SVT-AV1 in this FFmpeg".into()))?;
    let mut octx = format::output_as(dest, "avif").map_err(avif_err("muxer"))?;
    let mut ctx = codec::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(avif_err("encoder"))?;
    let time_base = Rational(1, 1000);
    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(Pixel::YUV420P);
    ctx.set_time_base(time_base);
    // Timed in milliseconds, but SVT-AV1 reads the rate from the time base unless told.
    ctx.set_frame_rate(Some(Rational(AnimFormat::Avif.max_fps() as i32, 1)));
    ctx.set_color_range(Range::JPEG);
    ctx.set_colorspace(Space::BT470BG);
    ctx.set_color_primaries(Primaries::BT709);
    ctx.set_color_transfer_characteristic(TransferCharacteristic::IEC61966_2_1);
    if octx.format().flags().contains(format::Flags::GLOBAL_HEADER) {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    // libavif's mapping: quality 0..=100 to a quantizer 63..=0.
    let quality = i32::from(settings.avif.quality.min(100));
    let quantizer = (((100 - quality) * 63 + 50) / 100).max(1);
    let mut options = ffmpeg_next::Dictionary::new();
    options.set("crf", &quantizer.to_string());
    // SVT-AV1's preset 8: an animation is encoded whole, at the speed of a short video.
    options.set("preset", "8");
    let encoder = ctx.open_with(options).map_err(avif_err("encoder"))?;
    let mut stream = octx.add_stream(codec).map_err(avif_err("stream"))?;
    stream.set_parameters(&encoder);
    stream.set_time_base(time_base);
    octx.write_header().map_err(avif_err("header"))?;
    let time_base = octx.stream(0).map_or(time_base, |s| s.time_base());
    Ok(Inner::Avif {
        octx,
        encoder,
        time_base,
    })
}

/// Writes what the encoder has ready.
fn drain(
    encoder: &mut encoder::Video,
    octx: &mut format::context::Output,
    time_base: Rational,
) -> Result<(), ImageError> {
    let mut packet = Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(0);
        packet.rescale_ts(Rational(1, 1000), time_base);
        packet.write_interleaved(octx).map_err(avif_err("write"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square moving across a gradient, ten frames at 100 ms.
    fn frames() -> Vec<Vec<u8>> {
        let (w, h) = (64usize, 48usize);
        (0..10)
            .map(|f| {
                let mut px = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    for x in 0..w {
                        let inside = (f * 5..f * 5 + 12).contains(&x) && (10..22).contains(&y);
                        let v = if inside { 255 } else { (x * 4) as u8 };
                        px.extend_from_slice(&[v, (y * 5) as u8, 255 - v, 255]);
                    }
                }
                px
            })
            .collect()
    }

    fn write(format: AnimFormat) -> Vec<u8> {
        let dest = std::env::temp_dir().join(format!(
            "vixeeny-anim-{}-{}.{}",
            std::process::id(),
            format.extension(),
            format.extension()
        ));
        let mut anim = Animation::new(format, 64, 48, &Settings::default(), &dest).unwrap();
        for (i, f) in frames().iter().enumerate() {
            anim.push(f, i as u64 * 100).unwrap();
        }
        anim.finish(1000).unwrap();
        let bytes = std::fs::read(&dest).unwrap();
        let _ = std::fs::remove_file(&dest);
        bytes
    }

    #[test]
    fn a_gif_holds_every_frame_and_lasts_the_clip() {
        let bytes = write(AnimFormat::Gif);
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(bytes.as_slice()).unwrap();
        let (mut count, mut hundredths) = (0, 0u32);
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            count += 1;
            hundredths += u32::from(frame.delay);
            assert_eq!((frame.width, frame.height), (64, 48));
        }
        assert_eq!((count, hundredths), (10, 100));
        assert_eq!(decoder.repeat(), gif::Repeat::Infinite);
    }

    #[test]
    fn an_animated_webp_is_one() {
        let bytes = write(AnimFormat::WebP);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        let has = |tag: &[u8]| bytes.windows(4).any(|w| w == tag);
        assert!(has(b"ANIM") && has(b"ANMF"));
    }

    #[test]
    fn an_animated_avif_is_an_image_sequence() {
        let bytes = write(AnimFormat::Avif);
        assert_eq!(&bytes[4..8], b"ftyp");
        // `avis`: the brand of AVIF image sequences.
        assert!(
            bytes[8..64.min(bytes.len())]
                .windows(4)
                .any(|w| w == b"avis")
        );
    }

    #[test]
    fn animations_fit_their_box_on_even_sides() {
        assert_eq!(fit(1920, 1080, 640), (640, 360));
        assert_eq!(fit(1080, 1920, 640), (360, 640));
        assert_eq!(fit(301, 201, 640), (300, 200));
        assert_eq!(fit(2560, 1080, 1280), (1280, 540));
    }

    #[test]
    fn frames_are_picked_one_per_slot() {
        let times: Vec<f64> = (0..12).map(|i| f64::from(i) / 60.0).collect();
        assert_eq!(pick(&times, 15.0), [0, 4, 8]);
        assert_eq!(pick(&times, 60.0), (0..12).collect::<Vec<_>>());
        // A clip slower than the animation: every frame.
        let slow: Vec<f64> = (0..4).map(|i| f64::from(i) / 10.0).collect();
        assert_eq!(pick(&slow, 15.0), [0, 1, 2, 3]);
        assert!(pick(&[], 15.0).is_empty());
    }

    #[test]
    fn gif_delays_add_up_to_the_clip() {
        // 15 fps: 66.7 ms per frame, as 7, 6, 7… hundredths.
        let starts: Vec<u64> = (0..15).map(|i| i * 1000 / 15).collect();
        let delays = gif_delays(&starts, 1000);
        assert_eq!(delays.iter().map(|&d| u32::from(d)).sum::<u32>(), 100);
        assert!(delays.iter().all(|&d| (6..=7).contains(&d)));
        // Browsers slow down anything under 2 hundredths.
        assert_eq!(gif_delays(&[0], 5), [2]);
    }
}
