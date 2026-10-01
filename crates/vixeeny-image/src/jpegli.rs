// SPDX-License-Identifier: GPL-3.0-or-later
//! JPEG through jpegli (libjpeg-compatible C API, `jpegli_*` functions). Its libjpeg-style
//! structures come from the generated bindings; the few functions used are declared here
//! because the C++ header cannot be fed to bindgen.

use std::ffi::{c_int, c_uint, c_ulong, c_void};
use std::ptr;

use crate::native::{
    J_COLOR_SPACE_JCS_RGB as JCS_RGB, JPEG_LIB_VERSION, JSAMPARRAY, JSAMPROW, jpeg_compress_struct,
    jpeg_error_mgr,
};
use crate::{Bgra, Chroma, ImageError, JpegSettings, icc};

unsafe extern "C" {
    fn jpegli_std_error(err: *mut jpeg_error_mgr) -> *mut jpeg_error_mgr;
    fn jpegli_CreateCompress(cinfo: *mut jpeg_compress_struct, version: c_int, structsize: usize);
    fn jpegli_mem_dest(cinfo: *mut jpeg_compress_struct, buffer: *mut *mut u8, size: *mut c_ulong);
    fn jpegli_set_defaults(cinfo: *mut jpeg_compress_struct);
    fn jpegli_set_quality(cinfo: *mut jpeg_compress_struct, quality: c_int, force_baseline: c_int);
    fn jpegli_simple_progression(cinfo: *mut jpeg_compress_struct);
    fn jpegli_start_compress(cinfo: *mut jpeg_compress_struct, write_all_tables: c_int);
    fn jpegli_write_icc_profile(cinfo: *mut jpeg_compress_struct, data: *const u8, len: c_uint);
    fn jpegli_write_scanlines(
        cinfo: *mut jpeg_compress_struct,
        rows: JSAMPARRAY,
        n: c_uint,
    ) -> c_uint;
    fn jpegli_finish_compress(cinfo: *mut jpeg_compress_struct);
    fn jpegli_destroy_compress(cinfo: *mut jpeg_compress_struct);
    fn free(ptr: *mut c_void);
}

/// libjpeg reports fatal errors through this callback and expects it not to return. Unwinding
/// through C++ frames is not an option, so a failure ends the process; inputs are validated
/// beforehand (size, channels), which keeps this a last resort.
unsafe extern "C" fn fatal(_: *mut crate::native::jpeg_common_struct) {
    eprintln!("vixeeny: fatal jpegli error");
    std::process::abort();
}

pub(crate) fn encode(image: &Bgra<'_>, settings: &JpegSettings) -> Result<Vec<u8>, ImageError> {
    if image.width > 65_535 || image.height > 65_535 {
        return Err(ImageError::Jpeg(
            "JPEG is limited to 65535 pixels per side".into(),
        ));
    }
    let profile = icc::srgb()?;
    let rgb = image.to_rgb();
    let row_bytes = image.width as usize * 3;
    let mut rows: Vec<JSAMPROW> = rgb
        .chunks_exact(row_bytes)
        .map(|r| r.as_ptr().cast_mut())
        .collect();

    // SAFETY: `cinfo` and `err` live for the whole block; the libjpeg call sequence is the
    // documented one (create, destination, parameters, start, scanlines, finish, destroy).
    // `rows` points into `rgb`, which outlives the compression; libjpeg only reads them.
    unsafe {
        let mut err = std::mem::MaybeUninit::<jpeg_error_mgr>::zeroed();
        let mut cinfo = std::mem::MaybeUninit::<jpeg_compress_struct>::zeroed();
        let cinfo_ptr = cinfo.as_mut_ptr();
        (*cinfo_ptr).err = jpegli_std_error(err.as_mut_ptr());
        (*err.as_mut_ptr()).error_exit = Some(fatal);
        jpegli_CreateCompress(
            cinfo_ptr,
            JPEG_LIB_VERSION as c_int,
            size_of::<jpeg_compress_struct>(),
        );

        let mut buffer: *mut u8 = ptr::null_mut();
        let mut size: c_ulong = 0;
        jpegli_mem_dest(cinfo_ptr, &raw mut buffer, &raw mut size);

        (*cinfo_ptr).image_width = image.width;
        (*cinfo_ptr).image_height = image.height;
        (*cinfo_ptr).input_components = 3;
        (*cinfo_ptr).in_color_space = JCS_RGB;
        jpegli_set_defaults(cinfo_ptr);
        jpegli_set_quality(cinfo_ptr, c_int::from(settings.quality.clamp(1, 100)), 1);
        // Luma sampling factors decide the chroma subsampling: 2×2 is 4:2:0, 1×1 is 4:4:4.
        let luma_factor = if settings.chroma == Chroma::Yuv420 {
            2
        } else {
            1
        };
        let comp = (*cinfo_ptr).comp_info;
        (*comp).h_samp_factor = luma_factor;
        (*comp).v_samp_factor = luma_factor;
        if settings.progressive {
            jpegli_simple_progression(cinfo_ptr);
        }
        jpegli_start_compress(cinfo_ptr, 1);
        jpegli_write_icc_profile(cinfo_ptr, profile.as_ptr(), profile.len() as c_uint);
        let mut done = 0usize;
        while done < rows.len() {
            let n = jpegli_write_scanlines(
                cinfo_ptr,
                rows.as_mut_ptr().add(done),
                (rows.len() - done) as c_uint,
            );
            if n == 0 {
                jpegli_destroy_compress(cinfo_ptr);
                free(buffer.cast());
                return Err(ImageError::Jpeg("no scanline accepted".into()));
            }
            done += n as usize;
        }
        jpegli_finish_compress(cinfo_ptr);
        let out = std::slice::from_raw_parts(buffer, size as usize).to_vec();
        jpegli_destroy_compress(cinfo_ptr);
        free(buffer.cast());
        Ok(out)
    }
}
