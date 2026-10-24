// SPDX-License-Identifier: GPL-3.0-or-later
//! JPEG XL through libjxl.

use std::ffi::c_void;
use std::ptr;

use crate::native as ffi;
use crate::{Bgra, Hdr, ImageError, JxlSettings};

struct Encoder(*mut ffi::JxlEncoder);
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by `JxlEncoderCreate`, destroyed once.
        unsafe { ffi::JxlEncoderDestroy(self.0) };
    }
}

struct Runner(*mut c_void);
impl Drop for Runner {
    fn drop(&mut self) {
        // SAFETY: created by `JxlThreadParallelRunnerCreate`, destroyed once after the encoder.
        unsafe { ffi::JxlThreadParallelRunnerDestroy(self.0) };
    }
}

fn check(status: ffi::JxlEncoderStatus, what: &str) -> Result<(), ImageError> {
    if status == ffi::JxlEncoderStatus_JXL_ENC_SUCCESS {
        Ok(())
    } else {
        Err(ImageError::Jxl(format!("{what} (status {status})")))
    }
}

pub(crate) fn encode(image: &Bgra<'_>, settings: &JxlSettings) -> Result<Vec<u8>, ImageError> {
    let rgb = image.to_rgb();
    let mut srgb = ffi::JxlColorEncoding::default();
    // SAFETY: `srgb` is a valid out-pointer.
    unsafe { ffi::JxlColorEncodingSetToSRGB(&raw mut srgb, 0) };
    let samples = Samples {
        width: image.width,
        height: image.height,
        bits: 8,
        data_type: ffi::JxlDataType_JXL_TYPE_UINT8,
        bytes: &rgb,
        intensity_target: 0.0,
    };
    write(&samples, &srgb, settings)
}

/// 16 bits per sample, BT.2020 / PQ, with the brightest light it holds as its intensity target:
/// the peak viewers tone map from, rather than the 10 000 nits PQ can hold.
pub(crate) fn encode_hdr(image: &Hdr<'_>, settings: &JxlSettings) -> Result<Vec<u8>, ImageError> {
    // Native endianness, as the pixel format of `write` says.
    let bytes = image.to_pq_bytes(u16::to_ne_bytes);
    let colour = ffi::JxlColorEncoding {
        color_space: ffi::JxlColorSpace_JXL_COLOR_SPACE_RGB,
        white_point: ffi::JxlWhitePoint_JXL_WHITE_POINT_D65,
        primaries: ffi::JxlPrimaries_JXL_PRIMARIES_2100,
        transfer_function: ffi::JxlTransferFunction_JXL_TRANSFER_FUNCTION_PQ,
        rendering_intent: ffi::JxlRenderingIntent_JXL_RENDERING_INTENT_RELATIVE,
        ..ffi::JxlColorEncoding::default()
    };
    let samples = Samples {
        width: image.width,
        height: image.height,
        bits: 16,
        data_type: ffi::JxlDataType_JXL_TYPE_UINT16,
        bytes: &bytes,
        intensity_target: image.light().max_cll.max(image.sdr_white_nits),
    };
    write(&samples, &colour, settings)
}

/// Tightly packed RGB.
struct Samples<'a> {
    width: u32,
    height: u32,
    bits: u32,
    data_type: ffi::JxlDataType,
    bytes: &'a [u8],
    /// The brightest light, in nits; 0 lets libjxl choose from the colour encoding.
    intensity_target: f32,
}

fn write(
    samples: &Samples<'_>,
    colour: &ffi::JxlColorEncoding,
    settings: &JxlSettings,
) -> Result<Vec<u8>, ImageError> {
    // SAFETY: libjxl objects are created and destroyed through the RAII wrappers; the samples
    // and the structs passed by pointer outlive the calls that read them (libjxl copies the
    // pixel data in `JxlEncoderAddImageFrame`).
    unsafe {
        // The runner must outlive the encoder: declared first, dropped last.
        let runner = Runner(ffi::JxlThreadParallelRunnerCreate(
            ptr::null(),
            ffi::JxlThreadParallelRunnerDefaultNumWorkerThreads(),
        ));
        let enc = Encoder(ffi::JxlEncoderCreate(ptr::null()));
        if enc.0.is_null() || runner.0.is_null() {
            return Err(ImageError::Jxl("out of memory".into()));
        }
        check(
            ffi::JxlEncoderSetParallelRunner(enc.0, Some(ffi::JxlThreadParallelRunner), runner.0),
            "parallel runner",
        )?;

        let mut info = ffi::JxlBasicInfo::default();
        ffi::JxlEncoderInitBasicInfo(&raw mut info);
        info.xsize = samples.width;
        info.ysize = samples.height;
        info.bits_per_sample = samples.bits;
        info.num_color_channels = 3;
        info.alpha_bits = 0;
        info.uses_original_profile = i32::from(settings.lossless);
        if samples.intensity_target > 0.0 {
            info.intensity_target = samples.intensity_target;
        }
        check(
            ffi::JxlEncoderSetBasicInfo(enc.0, &raw const info),
            "basic info",
        )?;

        check(
            ffi::JxlEncoderSetColorEncoding(enc.0, colour),
            "colour encoding",
        )?;

        let frame = ffi::JxlEncoderFrameSettingsCreate(enc.0, ptr::null());
        check(
            ffi::JxlEncoderFrameSettingsSetOption(
                frame,
                ffi::JxlEncoderFrameSettingId_JXL_ENC_FRAME_SETTING_EFFORT,
                i64::from(settings.effort.clamp(1, 9)),
            ),
            "effort",
        )?;
        if settings.lossless {
            check(ffi::JxlEncoderSetFrameLossless(frame, 1), "lossless")?;
        } else {
            check(
                ffi::JxlEncoderSetFrameDistance(frame, settings.distance.clamp(0.0, 25.0)),
                "distance",
            )?;
        }
        let format = ffi::JxlPixelFormat {
            num_channels: 3,
            data_type: samples.data_type,
            endianness: ffi::JxlEndianness_JXL_NATIVE_ENDIAN,
            align: 0,
        };
        check(
            ffi::JxlEncoderAddImageFrame(
                frame,
                &raw const format,
                samples.bytes.as_ptr().cast::<c_void>(),
                samples.bytes.len(),
            ),
            "add frame",
        )?;
        ffi::JxlEncoderCloseInput(enc.0);

        let mut out = vec![0u8; 64 * 1024];
        let mut written = 0usize;
        loop {
            let mut next = out.as_mut_ptr().add(written);
            let mut avail = out.len() - written;
            let status = ffi::JxlEncoderProcessOutput(enc.0, &raw mut next, &raw mut avail);
            written = out.len() - avail;
            if status == ffi::JxlEncoderStatus_JXL_ENC_NEED_MORE_OUTPUT {
                out.resize(out.len() * 2, 0);
            } else {
                check(status, "process output")?;
                break;
            }
        }
        out.truncate(written);
        Ok(out)
    }
}
