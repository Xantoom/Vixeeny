// SPDX-License-Identifier: GPL-3.0-or-later
//! JPEG through the pure-Rust `jpeg-encoder` for now; jpegli (plan 3) replaces it once its FFI
//! lands on the three platforms (see the decision log).

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

use crate::{Bgra, Chroma, ImageError, JpegSettings, icc};

pub(crate) fn encode(image: &Bgra<'_>, settings: &JpegSettings) -> Result<Vec<u8>, ImageError> {
    let (width, height) = match (u16::try_from(image.width), u16::try_from(image.height)) {
        (Ok(w), Ok(h)) => (w, h),
        _ => {
            return Err(ImageError::Jpeg(
                "JPEG is limited to 65535 pixels per side".into(),
            ));
        }
    };
    let rgb = image.to_rgb();
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, settings.quality.clamp(1, 100));
    encoder.set_sampling_factor(match settings.chroma {
        Chroma::Yuv444 => SamplingFactor::F_1_1,
        Chroma::Yuv420 => SamplingFactor::F_2_2,
    });
    encoder.set_progressive(settings.progressive);
    encoder
        .add_icc_profile(icc::srgb()?)
        .map_err(|e| ImageError::Jpeg(e.to_string()))?;
    encoder
        .encode(&rgb, width, height, ColorType::Rgb)
        .map_err(|e| ImageError::Jpeg(e.to_string()))?;
    Ok(out)
}
