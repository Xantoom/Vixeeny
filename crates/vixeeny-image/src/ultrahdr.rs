// SPDX-License-Identifier: GPL-3.0-or-later
//! JPEG Ultra HDR through libultrahdr: the SDR rendering (tone-mapped, encoded by jpegli) is
//! the picture every viewer shows; a gain map computed from the HDR capture lets HDR screens
//! restore its highlights.

use std::ptr;

use vixeeny_common::{colour, half};

use crate::native::*;
use crate::{Bgra, Hdr, ImageError, JpegSettings};

struct Encoder(*mut uhdr_codec_private_t);
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by `uhdr_create_encoder`, released once.
        unsafe { uhdr_release_encoder(self.0) };
    }
}

fn check(status: uhdr_error_info_t, what: &str) -> Result<(), ImageError> {
    if status.error_code == uhdr_codec_err_UHDR_CODEC_OK {
        return Ok(());
    }
    let detail = if status.has_detail != 0 {
        let bytes: Vec<u8> = status
            .detail
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        format!("error {}", status.error_code)
    };
    Err(ImageError::Jpeg(format!("ultra hdr {what}: {detail}")))
}

/// libultrahdr's linear half floats: `1.0` is 203 nits (BT.2408 graphics white).
const UHDR_WHITE_NITS: f32 = 203.0;

pub(crate) fn encode(image: &Hdr<'_>, settings: &JpegSettings) -> Result<Vec<u8>, ImageError> {
    let sdr = image.to_sdr();
    let stride = image.width as usize * 4;
    let base = crate::jpegli::encode(
        &Bgra::new(image.width, image.height, stride, &sdr),
        settings,
    )?;
    drop(sdr);
    let pixels = image.width as usize * image.height as usize;
    let scale = 80.0 / UHDR_WHITE_NITS;
    let top = 10_000.0 / UHDR_WHITE_NITS;
    let halves: Vec<u16> = image.rgba[..pixels * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| {
            let [r, g, b] = colour::into_gamut([px[0], px[1], px[2]]);
            [r, g, b, 1.0].map(|c| half::from_f32((c * scale).clamp(0.0, top)))
        })
        .collect();

    // SAFETY: the encoder is released by its RAII wrapper; the images it is given point into
    // `halves` and `base`, which outlive `uhdr_encode`; the encoded stream belongs to the
    // encoder and is copied out before it is released.
    unsafe {
        let enc = Encoder(uhdr_create_encoder());
        if enc.0.is_null() {
            return Err(ImageError::Jpeg("ultra hdr: out of memory".into()));
        }
        let mut hdr = uhdr_raw_image_t {
            fmt: uhdr_img_fmt_UHDR_IMG_FMT_64bppRGBAHalfFloat,
            cg: uhdr_color_gamut_UHDR_CG_BT_709,
            ct: uhdr_color_transfer_UHDR_CT_LINEAR,
            range: uhdr_color_range_UHDR_CR_FULL_RANGE,
            w: image.width,
            h: image.height,
            planes: [
                halves.as_ptr().cast_mut().cast(),
                ptr::null_mut(),
                ptr::null_mut(),
            ],
            stride: [image.width, 0, 0],
        };
        check(
            uhdr_enc_set_raw_image(enc.0, &raw mut hdr, uhdr_img_label_UHDR_HDR_IMG),
            "hdr picture",
        )?;
        let mut sdr = uhdr_compressed_image_t {
            data: base.as_ptr().cast_mut().cast(),
            data_sz: base.len(),
            capacity: base.len(),
            cg: uhdr_color_gamut_UHDR_CG_BT_709,
            ct: uhdr_color_transfer_UHDR_CT_SRGB,
            range: uhdr_color_range_UHDR_CR_FULL_RANGE,
        };
        // Same codec in and out: jpegli's JPEG becomes the primary picture as it is.
        check(
            uhdr_enc_set_compressed_image(enc.0, &raw mut sdr, uhdr_img_label_UHDR_SDR_IMG),
            "sdr picture",
        )?;
        check(
            uhdr_enc_set_quality(
                enc.0,
                i32::from(settings.quality.clamp(1, 100)),
                uhdr_img_label_UHDR_GAIN_MAP_IMG,
            ),
            "quality",
        )?;
        check(
            uhdr_enc_set_target_display_peak_brightness(
                enc.0,
                image.peak_nits.clamp(UHDR_WHITE_NITS, 10_000.0),
            ),
            "peak brightness",
        )?;
        check(uhdr_encode(enc.0), "encode")?;
        let out = uhdr_get_encoded_stream(enc.0);
        if out.is_null() || (*out).data.is_null() {
            return Err(ImageError::Jpeg("ultra hdr: no output".into()));
        }
        Ok(std::slice::from_raw_parts((*out).data.cast::<u8>(), (*out).data_sz).to_vec())
    }
}
