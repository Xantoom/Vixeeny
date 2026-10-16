// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading the images Vixeeny writes (for the notification thumbnails): PNG, JPEG, WebP, AVIF and
//! JPEG XL. The result is opaque sRGB BGRA, upright: the embedded colour profile and
//! the orientation (EXIF, or `irot`/`imir` for AVIF) are applied, transparency is flattened on
//! white (the encoders ignore alpha).

use std::io::Cursor;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use moxcms::{ColorProfile, Layout, TransformOptions};

use crate::{ImageError, icc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Png,
    Jpeg,
    WebP,
    Avif,
    Jxl,
}

impl SourceFormat {
    /// The format of `bytes`, from the magic numbers.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        let starts = |p: &[u8]| bytes.starts_with(p);
        if starts(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if starts(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::WebP)
        } else if bytes.len() >= 12
            && &bytes[4..8] == b"ftyp"
            && matches!(&bytes[8..12], b"avif" | b"avis")
        {
            Some(Self::Avif)
        } else if starts(&[0xFF, 0x0A]) || starts(b"\0\0\0\x0cJXL \r\n\x87\n") {
            Some(Self::Jxl)
        } else {
            None
        }
    }
}

/// An upright, opaque sRGB picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA, alpha 255.
    pub bgra: Vec<u8>,
}

fn bad(e: impl std::fmt::Display) -> ImageError {
    ImageError::Decode(e.to_string())
}

pub fn decode(bytes: &[u8]) -> Result<Decoded, ImageError> {
    match SourceFormat::sniff(bytes).ok_or_else(|| bad("unrecognised image format"))? {
        SourceFormat::Jxl => decode_jxl(bytes),
        SourceFormat::Avif => decode_avif(bytes),
        SourceFormat::Png => match decode_png_pq(bytes)? {
            Some(decoded) => Ok(decoded),
            None => decode_with_image(bytes),
        },
        _ => decode_with_image(bytes),
    }
}

fn decode_with_image(bytes: &[u8]) -> Result<Decoded, ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(bad)?;
    let mut decoder = reader.into_decoder().map_err(bad)?;
    let profile = decoder.icc_profile().map_err(bad)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut picture = DynamicImage::from_decoder(decoder).map_err(bad)?;
    picture.apply_orientation(orientation);
    let rgba = picture.into_rgba8();
    Ok(finish(
        rgba.width(),
        rgba.height(),
        rgba.into_raw(),
        profile.as_deref(),
    ))
}

/// A PNG whose `cICP` chunk says BT.2020 / PQ (what Vixeeny writes for HDR), tone-mapped to SDR;
/// `None` for every other PNG.
fn decode_png_pq(bytes: &[u8]) -> Result<Option<Decoded>, ImageError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(bad)?;
    let pq = reader
        .info()
        .coding_independent_code_points
        .is_some_and(|c| c.color_primaries == 9 && c.transfer_function == 16);
    if !pq {
        return Ok(None);
    }
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| bad("image too large"))?
    ];
    let info = reader.next_frame(&mut buf).map_err(bad)?;
    let (width, height) = (info.width, info.height);
    let samples = &buf[..info.buffer_size()];
    let signal: Vec<[f32; 3]> = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgb, png::BitDepth::Sixteen) => samples
            .as_chunks::<6>()
            .0
            .iter()
            .map(|p| {
                let c = |i: usize| f32::from(u16::from_be_bytes([p[i], p[i + 1]])) / 65535.0;
                [c(0), c(2), c(4)]
            })
            .collect(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => samples
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(|c| f32::from(c) / 255.0))
            .collect(),
        (png::ColorType::Grayscale, png::BitDepth::Sixteen) => samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| [f32::from(u16::from_be_bytes(*p)) / 65535.0; 3])
            .collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => {
            samples.iter().map(|&c| [f32::from(c) / 255.0; 3]).collect()
        }
        _ => return Ok(None),
    };
    Ok(Some(Decoded {
        width,
        height,
        bgra: crate::hdr::pq_to_sdr(width, height, &signal),
    }))
}

fn decode_jxl(bytes: &[u8]) -> Result<Decoded, ImageError> {
    use jxl_oxide::{EnumColourEncoding, JxlImage, Moxcms, RenderingIntent};
    let mut image = JxlImage::builder().read(Cursor::new(bytes)).map_err(bad)?;
    image.set_cms(Moxcms);
    image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    let render = image.render_frame(0).map_err(bad)?;
    let fb = render.image_all_channels();
    let (w, h, channels) = (fb.width(), fb.height(), fb.channels());
    let quantise = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for px in fb.buf().chunks_exact(channels) {
        match channels {
            1 => rgba.extend_from_slice(&[quantise(px[0]); 3]),
            2 => rgba.extend_from_slice(&[
                quantise(px[0]),
                quantise(px[0]),
                quantise(px[0]),
                quantise(px[1]),
            ]),
            3 => rgba.extend_from_slice(&[quantise(px[0]), quantise(px[1]), quantise(px[2]), 255]),
            4 => rgba.extend_from_slice(&[
                quantise(px[0]),
                quantise(px[1]),
                quantise(px[2]),
                quantise(px[3]),
            ]),
            _ => return Err(bad("unsupported JPEG XL colour model")),
        }
    }
    // jxl-oxide has converted to sRGB and applied the orientation.
    Ok(finish(w as u32, h as u32, rgba, None))
}

fn decode_avif(bytes: &[u8]) -> Result<Decoded, ImageError> {
    let d = crate::avif::decode(bytes)?;
    Ok(finish(d.width, d.height, d.rgba, None))
}

/// Profile → sRGB, then alpha flattened on white, then BGRA.
fn finish(width: u32, height: u32, mut rgba: Vec<u8>, profile: Option<&[u8]>) -> Decoded {
    if let Some(profile) = profile {
        convert_to_srgb(&mut rgba, profile);
    }
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8;
            }
        }
        px.swap(0, 2);
        px[3] = 255;
    }
    Decoded {
        width,
        height,
        bgra: rgba,
    }
}

/// Leaves the pixels alone when the profile is unusable (a wrong profile must not lose the file).
fn convert_to_srgb(rgba: &mut [u8], profile: &[u8]) {
    if icc::srgb().is_ok_and(|s| s == profile) {
        return;
    }
    let Ok(source) = ColorProfile::new_from_slice(profile) else {
        return;
    };
    let Ok(transform) = source.create_transform_8bit(
        Layout::Rgba,
        &ColorProfile::new_srgb(),
        Layout::Rgba,
        TransformOptions::default(),
    ) else {
        return;
    };
    let mut out = vec![0u8; rgba.len()];
    if transform.transform(rgba, &mut out).is_ok() {
        // The transform does not carry alpha reliably across versions: keep the source's.
        for (dst, src) in out
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(rgba.as_chunks::<4>().0)
        {
            dst[3] = src[3];
        }
        rgba.copy_from_slice(&out);
    }
}
