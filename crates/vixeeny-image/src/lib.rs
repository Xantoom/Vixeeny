// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-image — image encoding (plan 5.4) and HDR → SDR tone mapping.
//!
//! Input is always opaque 8-bit BGRA ([`Bgra`], what the capture layer hands over). Every
//! encoder embeds an sRGB colour description and no other metadata.

#[cfg(feature = "native-codecs")]
mod avif;
mod decode;
mod icc;
#[cfg(feature = "native-codecs")]
mod jpegli;
#[cfg(feature = "native-codecs")]
mod jxl;
mod pixels;
mod png_enc;
pub mod tonemap;
mod webp;

pub use decode::{Decoded, SourceFormat, decode};
pub use pixels::Bgra;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
    Avif,
    Jxl,
}

impl ImageFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
            Self::Avif => "avif",
            Self::Jxl => "jxl",
        }
    }

    /// Whether this build can encode the format (JPEG, AVIF and JPEG XL need `native-codecs`).
    pub const fn available(self) -> bool {
        match self {
            Self::Png | Self::WebP => true,
            Self::Jpeg | Self::Avif | Self::Jxl => cfg!(feature = "native-codecs"),
        }
    }

    /// Parses the `image.format` setting.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "webp" => Some(Self::WebP),
            "avif" => Some(Self::Avif),
            "jxl" => Some(Self::Jxl),
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
            // Full colour: screenshots are text and interfaces, which 4:2:0 fringes.
            chroma: Chroma::Yuv444,
            // A few percent smaller, at the same quality.
            progressive: true,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvifSettings {
    /// 0 to 100.
    pub quality: u8,
    /// SVT-AV1 preset 0 (slowest) to 10 (fastest).
    pub speed: u8,
    /// 8 or 10 bits per channel.
    pub depth: u8,
    pub chroma: Chroma,
}

impl Default for AvifSettings {
    /// Plan 5.4: quality 80, 10 bits, 4:4:4.
    fn default() -> Self {
        Self {
            quality: 80,
            speed: 6,
            depth: 10,
            chroma: Chroma::Yuv444,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JxlSettings {
    pub lossless: bool,
    /// Butteraugli distance for lossy mode (1.0 is visually lossless).
    pub distance: f32,
    /// Effort 1 (fast) to 9 (slow).
    pub effort: u8,
}

impl Default for JxlSettings {
    /// Plan 5.4: lossless, effort 7.
    fn default() -> Self {
        Self {
            lossless: true,
            distance: 1.0,
            effort: 7,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Settings {
    pub png: PngSettings,
    pub jpeg: JpegSettings,
    pub webp: WebpSettings,
    pub avif: AvifSettings,
    pub jxl: JxlSettings,
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
    #[error("avif: {0}")]
    Avif(String),
    #[error("cannot read the image: {0}")]
    Decode(String),
    #[error("jpeg xl: {0}")]
    Jxl(String),
    #[error("{0:?} encoding is not part of this build")]
    Unavailable(ImageFormat),
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
        ImageFormat::WebP => webp::encode(image, &settings.webp),
        #[cfg(feature = "native-codecs")]
        ImageFormat::Jpeg => jpegli::encode(image, &settings.jpeg),
        #[cfg(feature = "native-codecs")]
        ImageFormat::Avif => avif::encode(image, &settings.avif),
        #[cfg(feature = "native-codecs")]
        ImageFormat::Jxl => jxl::encode(image, &settings.jxl),
        #[cfg(not(feature = "native-codecs"))]
        ImageFormat::Jpeg | ImageFormat::Avif | ImageFormat::Jxl => {
            Err(ImageError::Unavailable(format))
        }
    }
}

#[cfg(test)]
mod decode_tests;
#[cfg(test)]
mod tests;

#[cfg(feature = "native-codecs")]
mod native;
