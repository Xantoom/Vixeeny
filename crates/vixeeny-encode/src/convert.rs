// SPDX-License-Identifier: GPL-3.0-or-later
//! Colour conversion of the frames read back to the CPU (full-range RGB → limited-range YUV),
//! through swscale's frame API: it reads the captured rows where they are (no copy into an
//! `AVFrame`) and spreads the work over every core (a 4K frame: 8 ms on one, 1.4 ms on 16).

use ffmpeg_next::format::Pixel;
use ffmpeg_next::{ffi, frame};

use crate::recorder::RecordError;

/// Rows of packed RGB in memory: `width × height` pixels of `format`, `stride` bytes apart.
pub(crate) struct Rows<'a> {
    pub bytes: &'a [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
    pub format: Pixel,
}

pub(crate) struct Converter {
    ctx: *mut ffi::SwsContext,
}

// SAFETY: the context is only used through `&mut self`, by one thread at a time.
unsafe impl Send for Converter {}

impl Converter {
    pub(crate) fn new() -> Result<Self, RecordError> {
        // SAFETY: plain allocation; checked for NULL below.
        let ctx = unsafe { ffi::sws_alloc_context() };
        if ctx.is_null() {
            return Err(RecordError::Config("cannot allocate a scaler".into()));
        }
        let this = Self { ctx };
        // SAFETY: `ctx` is a live context; the option names are NUL-terminated.
        unsafe {
            // 0: as many threads as cores.
            ffi::av_opt_set_int(ctx.cast(), c"threads".as_ptr(), 0, 0);
            ffi::av_opt_set_int(
                ctx.cast(),
                c"sws_flags".as_ptr(),
                i64::from(ffi::SwsFlags::SWS_BICUBIC as u32),
                0,
            );
        }
        Ok(this)
    }

    /// Converts `src` into `dst` (allocated, of the encoder's format and size), with the
    /// BT.2020 matrix for `hdr` and BT.709 otherwise.
    pub(crate) fn run(
        &mut self,
        src: &Rows<'_>,
        dst: &mut frame::Video,
        hdr: bool,
    ) -> Result<(), RecordError> {
        let len = src.stride * src.height.saturating_sub(1) as usize
            + src.width as usize * bytes_per_pixel(src.format);
        if src.height == 0 || src.bytes.len() < len {
            return Err(RecordError::Config("frame buffer too small".into()));
        }
        // SAFETY: the source frame only borrows `src.bytes` (its buffer frees nothing) and is
        // freed before this function returns; swscale reads `len` bytes from it (checked
        // above). `dst` is a live, allocated frame.
        unsafe {
            let mut input = ffi::av_frame_alloc();
            if input.is_null() {
                return Err(RecordError::Config("cannot allocate a frame".into()));
            }
            (*input).buf[0] = ffi::av_buffer_create(
                src.bytes.as_ptr().cast_mut(),
                len,
                Some(borrowed),
                std::ptr::null_mut(),
                ffi::AV_BUFFER_FLAG_READONLY,
            );
            (*input).data[0] = src.bytes.as_ptr().cast_mut();
            (*input).linesize[0] = src.stride as i32;
            (*input).width = src.width as i32;
            (*input).height = src.height as i32;
            (*input).format = ffi::AVPixelFormat::from(src.format) as i32;
            (*input).colorspace = ffi::AVColorSpace::AVCOL_SPC_RGB;
            (*input).color_range = ffi::AVColorRange::AVCOL_RANGE_JPEG;
            let out = dst.as_mut_ptr();
            (*out).colorspace = if hdr {
                ffi::AVColorSpace::AVCOL_SPC_BT2020_NCL
            } else {
                ffi::AVColorSpace::AVCOL_SPC_BT709
            };
            (*out).color_range = ffi::AVColorRange::AVCOL_RANGE_MPEG;
            let code = if (*input).buf[0].is_null() {
                ffi::AVERROR(ffi::ENOMEM)
            } else {
                ffi::sws_scale_frame(self.ctx, out, input)
            };
            ffi::av_frame_free(&raw mut input);
            if code < 0 {
                return Err(ffmpeg_next::Error::from(code).into());
            }
        }
        Ok(())
    }
}

impl Drop for Converter {
    fn drop(&mut self) {
        // SAFETY: the context was allocated by `sws_alloc_context`; this sets it to NULL.
        unsafe { ffi::sws_free_context(&raw mut self.ctx) };
    }
}

/// The source buffer belongs to the caller: nothing to free.
unsafe extern "C" fn borrowed(_: *mut std::ffi::c_void, _: *mut u8) {}

fn bytes_per_pixel(format: Pixel) -> usize {
    match format {
        Pixel::RGB48LE => 6,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use ffmpeg_next::software::scaling;

    use super::*;

    /// What the former single-threaded path gives, for comparison.
    fn legacy(src: &frame::Video, out: Pixel, hdr: bool) -> frame::Video {
        let (w, h) = (src.width(), src.height());
        let mut ctx =
            scaling::Context::get(src.format(), w, h, out, w, h, scaling::Flags::BICUBIC).unwrap();
        let matrix = if hdr {
            ffi::SWS_CS_BT2020
        } else {
            ffi::SWS_CS_ITU709
        };
        // SAFETY: live context, static tables.
        unsafe {
            let table = ffi::sws_getCoefficients(matrix);
            ffi::sws_setColorspaceDetails(
                ctx.as_mut_ptr(),
                table,
                1,
                table,
                0,
                0,
                1 << 16,
                1 << 16,
            );
        }
        let mut dst = frame::Video::new(out, w, h);
        ctx.run(src, &mut dst).unwrap();
        dst
    }

    fn same(a: &frame::Video, b: &frame::Video) {
        for plane in 0..a.planes() {
            let (sa, sb) = (a.stride(plane), b.stride(plane));
            let row = a.plane_width(plane) as usize
                * if a.format() == Pixel::YUV420P10LE {
                    2
                } else {
                    1
                };
            for y in 0..a.plane_height(plane) as usize {
                let ra = &a.data(plane)[y * sa..y * sa + row];
                let rb = &b.data(plane)[y * sb..y * sb + row];
                for (x, (p, q)) in ra.iter().zip(rb).enumerate() {
                    assert!(
                        (i32::from(*p) - i32::from(*q)).abs() <= 1,
                        "plane {plane}, row {y}, byte {x}: {p} vs {q}"
                    );
                }
            }
        }
    }

    fn check(input: Pixel, out: Pixel, hdr: bool) {
        ffmpeg_next::init().unwrap();
        let (w, h) = (96, 54);
        let mut src = frame::Video::new(input, w, h);
        let stride = src.stride(0);
        for (i, b) in src.data_mut(0).iter_mut().enumerate() {
            *b = (i * 7919 % 251) as u8;
        }
        let want = legacy(&src, out, hdr);
        let rows = Rows {
            bytes: src.data(0),
            stride,
            width: w,
            height: h,
            format: input,
        };
        let mut got = frame::Video::new(out, w, h);
        Converter::new().unwrap().run(&rows, &mut got, hdr).unwrap();
        same(&got, &want);
    }

    #[test]
    fn sdr_frames_convert_as_before() {
        check(Pixel::BGRA, Pixel::YUV420P, false);
        check(Pixel::BGRA, Pixel::YUV444P, false);
        check(Pixel::BGRA, Pixel::NV12, false);
    }

    #[test]
    fn hdr_frames_convert_as_before() {
        check(Pixel::RGB48LE, Pixel::YUV420P10LE, true);
    }

    #[test]
    fn a_short_buffer_is_refused() {
        ffmpeg_next::init().unwrap();
        let rows = Rows {
            bytes: &[0; 100],
            stride: 40,
            width: 10,
            height: 10,
            format: Pixel::BGRA,
        };
        let mut out = frame::Video::new(Pixel::YUV420P, 10, 10);
        assert!(
            Converter::new()
                .unwrap()
                .run(&rows, &mut out, false)
                .is_err()
        );
    }
}
