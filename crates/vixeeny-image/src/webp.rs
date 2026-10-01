// SPDX-License-Identifier: GPL-3.0-or-later
//! WebP through libwebp. The sRGB profile is added by wrapping the bitstream in an extended
//! (`VP8X`) container with an `ICCP` chunk.

use std::ffi::c_void;

use libwebp_sys::{
    WebPConfig, WebPEncode, WebPMemoryWrite, WebPMemoryWriter, WebPMemoryWriterClear,
    WebPMemoryWriterInit, WebPPicture, WebPPictureFree, WebPPictureImportRGB,
};

use crate::{Bgra, ImageError, WebpSettings, icc};

pub(crate) fn encode(image: &Bgra<'_>, settings: &WebpSettings) -> Result<Vec<u8>, ImageError> {
    let err = |what: &str| ImageError::WebP(what.to_owned());
    // WebP is limited to 16383 pixels per side.
    if image.width > 16383 || image.height > 16383 {
        return Err(err("WebP is limited to 16383 pixels per side"));
    }
    let rgb = image.to_rgb();
    let mut config = WebPConfig::new().map_err(|()| err("config init"))?;
    config.lossless = i32::from(settings.lossless);
    config.quality = settings.quality.clamp(0.0, 100.0);
    config.method = i32::from(settings.effort.min(6));
    // SAFETY: `config` is initialised.
    if unsafe { libwebp_sys::WebPValidateConfig(&raw const config) } == 0 {
        return Err(err("invalid config"));
    }
    let mut picture = WebPPicture::new().map_err(|()| err("picture init"))?;
    picture.use_argb = i32::from(settings.lossless);
    picture.width = image.width as i32;
    picture.height = image.height as i32;
    // SAFETY: `rgb` holds width × height tightly packed RGB pixels; libwebp copies them.
    let imported =
        unsafe { WebPPictureImportRGB(&raw mut picture, rgb.as_ptr(), image.width as i32 * 3) };
    if imported == 0 {
        // SAFETY: frees what the failed import may have allocated.
        unsafe { WebPPictureFree(&raw mut picture) };
        return Err(err("importing pixels"));
    }
    let mut writer = std::mem::MaybeUninit::<WebPMemoryWriter>::uninit();
    // SAFETY: Init fully initialises the writer; the picture only keeps a pointer to it for
    // the duration of the encode below, and both are cleaned up on every path.
    let bitstream = unsafe {
        WebPMemoryWriterInit(writer.as_mut_ptr());
        let mut writer = writer.assume_init();
        picture.writer = Some(WebPMemoryWrite);
        picture.custom_ptr = (&raw mut writer).cast::<c_void>();
        let ok = WebPEncode(&raw const config, &raw mut picture);
        let result = if ok == 0 {
            Err(ImageError::WebP(format!(
                "encoder error {:?}",
                picture.error_code
            )))
        } else {
            Ok(std::slice::from_raw_parts(writer.mem, writer.size).to_vec())
        };
        WebPMemoryWriterClear(&raw mut writer);
        WebPPictureFree(&raw mut picture);
        result
    }?;
    with_icc(&bitstream, image.width, image.height, icc::srgb()?)
}

fn chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
}

fn le24(v: u32) -> [u8; 3] {
    let b = v.to_le_bytes();
    [b[0], b[1], b[2]]
}

/// Wraps a simple-format WebP (`RIFF…WEBP` + image chunk) with `VP8X` and `ICCP`.
pub(crate) fn with_icc(
    webp: &[u8],
    width: u32,
    height: u32,
    profile: &[u8],
) -> Result<Vec<u8>, ImageError> {
    if webp.len() < 12 || &webp[0..4] != b"RIFF" || &webp[8..12] != b"WEBP" {
        return Err(ImageError::WebP("not a RIFF/WEBP stream".into()));
    }
    let mut vp8x = [0u8; 10];
    vp8x[0] = 0x20; // ICC profile present
    vp8x[4..7].copy_from_slice(&le24(width - 1));
    vp8x[7..10].copy_from_slice(&le24(height - 1));
    let mut body = Vec::with_capacity(webp.len() + profile.len() + 32);
    body.extend_from_slice(b"WEBP");
    chunk(&mut body, b"VP8X", &vp8x);
    chunk(&mut body, b"ICCP", profile);
    body.extend_from_slice(&webp[12..]);
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}
