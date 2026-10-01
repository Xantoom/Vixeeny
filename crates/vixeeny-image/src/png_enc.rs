// SPDX-License-Identifier: GPL-3.0-or-later
use png::{BitDepth, ColorType, Compression, Encoder, Filter, SrgbRenderingIntent};

use crate::{Bgra, ImageError, PngCompression, PngSettings};

/// RGB PNG tagged sRGB. Alpha is dropped: screen captures are opaque, and RGB compresses faster.
pub(crate) fn encode(image: &Bgra<'_>, settings: &PngSettings) -> Result<Vec<u8>, ImageError> {
    let rgb = image.to_rgb();
    let mut out = Vec::with_capacity(rgb.len() / 2);
    let mut encoder = Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(ColorType::Rgb);
    encoder.set_depth(BitDepth::Eight);
    encoder.set_source_srgb(SrgbRenderingIntent::Perceptual);
    let (compression, filter) = match settings.compression {
        PngCompression::Fast => (Compression::Fast, Filter::Sub),
        PngCompression::Default => (Compression::Balanced, Filter::Adaptive),
        PngCompression::High => (Compression::High, Filter::Adaptive),
    };
    encoder.set_compression(compression);
    encoder.set_filter(filter);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgb)?;
    writer.finish()?;

    match settings.oxipng_level {
        None => Ok(out),
        Some(level) => {
            let options = oxipng::Options::from_preset(level.min(6));
            oxipng::optimize_from_memory(&out, &options)
                .map_err(|e| ImageError::Oxipng(e.to_string()))
        }
    }
}
