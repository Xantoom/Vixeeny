// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading images for the converter (plan 5.5): PNG, JPEG, WebP, AVIF, JPEG XL, BMP, TIFF and
//! GIF (first frame). The result is opaque sRGB BGRA, upright: the embedded colour profile and
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
    Bmp,
    Tiff,
    Gif,
}

impl SourceFormat {
    /// File extensions the converter accepts.
    pub const EXTENSIONS: &'static [&'static str] = &[
        "png", "jpg", "jpeg", "jpe", "webp", "avif", "jxl", "bmp", "tif", "tiff", "gif",
    ];

    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            "webp" => Self::WebP,
            "avif" => Self::Avif,
            "jxl" => Self::Jxl,
            "bmp" => Self::Bmp,
            "tif" | "tiff" => Self::Tiff,
            "gif" => Self::Gif,
            _ => return None,
        })
    }

    /// Whether this build can read the format (AVIF needs `native-codecs`).
    pub const fn available(self) -> bool {
        !matches!(self, Self::Avif) || cfg!(feature = "native-codecs")
    }

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
        } else if starts(b"BM") {
            Some(Self::Bmp)
        } else if starts(b"II*\0") || starts(b"MM\0*") {
            Some(Self::Tiff)
        } else if starts(b"GIF8") {
            Some(Self::Gif)
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

impl Decoded {
    pub fn as_bgra(&self) -> crate::Bgra<'_> {
        crate::Bgra::new(self.width, self.height, self.width as usize * 4, &self.bgra)
    }
}

fn bad(e: impl std::fmt::Display) -> ImageError {
    ImageError::Decode(e.to_string())
}

pub fn decode(bytes: &[u8]) -> Result<Decoded, ImageError> {
    match SourceFormat::sniff(bytes).ok_or_else(|| bad("unrecognised image format"))? {
        SourceFormat::Jxl => decode_jxl(bytes),
        SourceFormat::Avif => decode_avif(bytes),
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

#[cfg(feature = "native-codecs")]
fn decode_avif(bytes: &[u8]) -> Result<Decoded, ImageError> {
    let d = crate::avif::decode(bytes)?;
    let mut picture = image::RgbaImage::from_raw(d.width, d.height, d.rgba)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| bad("inconsistent AVIF buffer"))?;
    for _ in 0..d.rotation {
        picture = picture.rotate270(); // anti-clockwise
    }
    match d.mirror {
        Some(0) => picture = picture.flipv(),
        Some(_) => picture = picture.fliph(),
        None => {}
    }
    let rgba = picture.into_rgba8();
    Ok(finish(
        rgba.width(),
        rgba.height(),
        rgba.into_raw(),
        d.icc.as_deref(),
    ))
}

#[cfg(not(feature = "native-codecs"))]
fn decode_avif(_: &[u8]) -> Result<Decoded, ImageError> {
    Err(ImageError::Unavailable(crate::ImageFormat::Avif))
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
