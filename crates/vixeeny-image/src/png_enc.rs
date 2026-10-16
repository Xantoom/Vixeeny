// SPDX-License-Identifier: GPL-3.0-or-later
use png::{BitDepth, ColorType, Compression, Encoder, Filter, SrgbRenderingIntent};

use crate::{Bgra, Hdr, ImageError, PngCompression, PngSettings};

/// RGB PNG tagged sRGB. Alpha is dropped: screen captures are opaque, and RGB compresses faster.
pub(crate) fn encode(image: &Bgra<'_>, settings: &PngSettings) -> Result<Vec<u8>, ImageError> {
    let rgb = image.to_rgb();
    write(image.width, image.height, &rgb, settings, false)
}

/// 16-bit RGB PNG, BT.2020 / PQ, described by a `cICP` chunk (PNG third edition).
pub(crate) fn encode_hdr(image: &Hdr<'_>, settings: &PngSettings) -> Result<Vec<u8>, ImageError> {
    let samples: Vec<u8> = image.to_pq().iter().flat_map(|c| c.to_be_bytes()).collect();
    write(image.width, image.height, &samples, settings, true)
}

/// BT.2020 primaries, PQ, RGB, full range (ITU-T H.273).
const CICP_BT2100_PQ: [u8; 4] = [9, 16, 0, 1];

fn write(
    width: u32,
    height: u32,
    rgb: &[u8],
    settings: &PngSettings,
    hdr: bool,
) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::with_capacity(rgb.len() / 2);
    let mut encoder = Encoder::new(&mut out, width, height);
    encoder.set_color(ColorType::Rgb);
    if hdr {
        encoder.set_depth(BitDepth::Sixteen);
    } else {
        encoder.set_depth(BitDepth::Eight);
        encoder.set_source_srgb(SrgbRenderingIntent::Perceptual);
    }
    let (compression, filter) = match settings.compression {
        PngCompression::Fast => (Compression::Fast, Filter::Sub),
        PngCompression::Default => (Compression::Balanced, Filter::Adaptive),
        PngCompression::High => (Compression::High, Filter::Adaptive),
    };
    encoder.set_compression(compression);
    encoder.set_filter(filter);
    let mut writer = encoder.write_header()?;
    if hdr {
        writer.write_chunk(png::chunk::cICP, &CICP_BT2100_PQ)?;
    }
    writer.write_image_data(rgb)?;
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
