// SPDX-License-Identifier: GPL-3.0-or-later
//! AVIF through libavif with SVT-AV1 as the encoder.

use std::ffi::CStr;

use crate::native::*;
use crate::{AvifSettings, Bgra, ImageError, icc};

fn check(result: avifResult, what: &str) -> Result<(), ImageError> {
    if result == avifResult_AVIF_RESULT_OK {
        return Ok(());
    }
    // SAFETY: `avifResultToString` returns a static NUL-terminated string.
    let text = unsafe { CStr::from_ptr(avifResultToString(result)) }.to_string_lossy();
    Err(ImageError::Avif(format!("{what}: {text}")))
}

struct Image(*mut avifImage);
impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `avifImageCreate` and is destroyed once.
        unsafe { avifImageDestroy(self.0) };
    }
}

struct Encoder(*mut avifEncoder);
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `avifEncoderCreate` and is destroyed once.
        unsafe { avifEncoderDestroy(self.0) };
    }
}

struct Output(avifRWData);
impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: `avifEncoderWrite` filled (or left empty) this buffer; Free handles both.
        unsafe { avifRWDataFree(&raw mut self.0) };
    }
}

pub(crate) fn encode(image: &Bgra<'_>, settings: &AvifSettings) -> Result<Vec<u8>, ImageError> {
    let depth = match settings.depth {
        8 => 8,
        10 => 10,
        other => return Err(ImageError::Avif(format!("unsupported depth {other}"))),
    };
    // The official SVT-AV1 only encodes 4:2:0 ("Only support 420 now"); 4:4:4 needs another
    // AV1 encoder (libaom or SVT-AV1-Tritium) and falls back to 4:2:0 until one is built in.
    let _requested = settings.chroma;
    let format = avifPixelFormat_AVIF_PIXEL_FORMAT_YUV420;
    let profile = icc::srgb()?;
    // SAFETY: every pointer below is created by libavif or points into data that outlives the
    // calls (`image.data`, `profile`); the wrappers free the C objects on all paths. libavif
    // only reads the RGB pixels during `avifImageRGBToYUV`.
    unsafe {
        let yuv = Image(avifImageCreate(image.width, image.height, depth, format));
        if yuv.0.is_null() {
            return Err(ImageError::Avif("out of memory".into()));
        }
        (*yuv.0).yuvRange = avifRange_AVIF_RANGE_FULL;
        (*yuv.0).colorPrimaries = AVIF_COLOR_PRIMARIES_BT709 as _;
        (*yuv.0).transferCharacteristics = AVIF_TRANSFER_CHARACTERISTICS_SRGB as _;
        (*yuv.0).matrixCoefficients = AVIF_MATRIX_COEFFICIENTS_BT601 as _;
        check(
            avifImageSetProfileICC(yuv.0, profile.as_ptr(), profile.len()),
            "icc profile",
        )?;

        let mut rgb = avifRGBImage::default();
        avifRGBImageSetDefaults(&raw mut rgb, yuv.0);
        rgb.format = avifRGBFormat_AVIF_RGB_FORMAT_BGRA;
        rgb.depth = 8;
        rgb.ignoreAlpha = 1;
        rgb.pixels = image.data.as_ptr().cast_mut();
        rgb.rowBytes =
            u32::try_from(image.stride).map_err(|_| ImageError::Avif("stride too large".into()))?;
        check(avifImageRGBToYUV(yuv.0, &raw const rgb), "rgb to yuv")?;

        let encoder = Encoder(avifEncoderCreate());
        if encoder.0.is_null() {
            return Err(ImageError::Avif("out of memory".into()));
        }
        (*encoder.0).codecChoice = avifCodecChoice_AVIF_CODEC_CHOICE_SVT;
        (*encoder.0).quality = i32::from(settings.quality.min(100));
        (*encoder.0).speed = i32::from(settings.speed.min(10));
        (*encoder.0).maxThreads = 0; // let libavif pick
        let mut output = Output(avifRWData::default());
        let written = avifEncoderWrite(encoder.0, yuv.0, &raw mut output.0);
        if written != avifResult_AVIF_RESULT_OK {
            let diag = CStr::from_ptr((*encoder.0).diag.error.as_ptr()).to_string_lossy();
            check(written, &format!("encode ({diag})"))?;
        }
        Ok(std::slice::from_raw_parts(output.0.data, output.0.size).to_vec())
    }
}

/// What libavif decoded: tightly packed RGBA8, the embedded profile and the display transform.
pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub icc: Option<Vec<u8>>,
    /// Anti-clockwise quarter turns (`irot`), then the mirror (`imir`), as stored in the file.
    pub rotation: u8,
    pub mirror: Option<u8>,
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Decoded, ImageError> {
    // SAFETY: standard decode sequence; objects are destroyed before returning, and the RGB
    // buffer is allocated by libavif and copied out before being freed.
    unsafe {
        let decoder = avifDecoderCreate();
        let image = avifImageCreateEmpty();
        let result = (|| {
            check(
                avifDecoderReadMemory(decoder, image, bytes.as_ptr(), bytes.len()),
                "decode",
            )?;
            let mut rgb = avifRGBImage::default();
            avifRGBImageSetDefaults(&raw mut rgb, image);
            rgb.format = avifRGBFormat_AVIF_RGB_FORMAT_RGBA;
            rgb.depth = 8;
            check(avifRGBImageAllocatePixels(&raw mut rgb), "alloc")?;
            let converted = check(avifImageYUVToRGB(image, &raw mut rgb), "yuv to rgb");
            let out = converted.map(|()| {
                let row = rgb.width as usize * 4;
                let mut v = Vec::with_capacity(row * rgb.height as usize);
                for y in 0..rgb.height as usize {
                    v.extend_from_slice(std::slice::from_raw_parts(
                        rgb.pixels.add(y * rgb.rowBytes as usize),
                        row,
                    ));
                }
                let img = &*image;
                let icc = (img.icc.size > 0 && !img.icc.data.is_null())
                    .then(|| std::slice::from_raw_parts(img.icc.data, img.icc.size).to_vec());
                let flags = img.transformFlags;
                Decoded {
                    width: rgb.width,
                    height: rgb.height,
                    rgba: v,
                    icc,
                    rotation: if flags & avifTransformFlag_AVIF_TRANSFORM_IROT != 0 {
                        img.irot.angle & 3
                    } else {
                        0
                    },
                    mirror: (flags & avifTransformFlag_AVIF_TRANSFORM_IMIR != 0)
                        .then_some(img.imir.axis),
                }
            });
            avifRGBImageFreePixels(&raw mut rgb);
            out
        })();
        avifImageDestroy(image);
        avifDecoderDestroy(decoder);
        result
    }
}

/// Test helper: tightly packed RGB8.
#[cfg(test)]
pub(crate) fn decode_rgb(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let d = decode(bytes)?;
    let rgb = d
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    Ok((d.width, d.height, rgb))
}
