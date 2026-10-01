// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-image — image encoding (plan 5.4) and HDR → SDR tone mapping.
//!
//! Input is always opaque 8-bit BGRA ([`Bgra`], what the capture layer hands over). Every
//! encoder embeds an sRGB colour description and no other metadata.

mod icc;
mod jpeg;
mod pixels;
mod png_enc;
pub mod tonemap;
mod webp;

pub use pixels::Bgra;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
}

impl ImageFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }

    /// Parses the `image.format` setting.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngCompression {
    Fast,
    Default,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PngSettings {
    pub compression: PngCompression,
    /// oxipng preset 0 (fast) to 6 (slow); `None` skips the optimisation pass.
    pub oxipng_level: Option<u8>,
}

impl Default for PngSettings {
    /// Plan 5.4: fast compression, oxipng off.
    fn default() -> Self {
        Self {
            compression: PngCompression::Fast,
            oxipng_level: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    Yuv444,
    Yuv420,
}

impl Chroma {
    /// Parses `444` / `420` as written in the config.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "444" => Some(Self::Yuv444),
            "420" => Some(Self::Yuv420),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegSettings {
    /// 1 to 100.
    pub quality: u8,
    pub chroma: Chroma,
    pub progressive: bool,
}

impl Default for JpegSettings {
    fn default() -> Self {
        Self {
            quality: 90,
            chroma: Chroma::Yuv444,
            progressive: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebpSettings {
    pub lossless: bool,
    /// 0 to 100 (for lossless: compression effort trade-off).
    pub quality: f32,
    /// Effort 0 (fast) to 6 (slow).
    pub effort: u8,
}

impl Default for WebpSettings {
    /// Plan 5.4: lossless.
    fn default() -> Self {
        Self {
            lossless: true,
            quality: 90.0,
            effort: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Settings {
    pub png: PngSettings,
    pub jpeg: JpegSettings,
    pub webp: WebpSettings,
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("invalid pixel buffer: {0}")]
    BadBuffer(&'static str),
    #[error("png: {0}")]
    Png(#[from] png::EncodingError),
    #[error("png optimisation: {0}")]
    Oxipng(String),
    #[error("jpeg: {0}")]
    Jpeg(String),
    #[error("webp: {0}")]
    WebP(String),
    #[error("colour profile: {0}")]
    Icc(String),
}

/// Encodes `image` in `format`.
pub fn encode(
    format: ImageFormat,
    image: &Bgra<'_>,
    settings: &Settings,
) -> Result<Vec<u8>, ImageError> {
    image.validate()?;
    match format {
        ImageFormat::Png => png_enc::encode(image, &settings.png),
        ImageFormat::Jpeg => jpeg::encode(image, &settings.jpeg),
        ImageFormat::WebP => webp::encode(image, &settings.webp),
    }
}

#[cfg(test)]
mod tests;
