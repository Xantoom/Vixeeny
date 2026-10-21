// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR frames for recording: the scRGB half-float frame WGC delivers on an HDR monitor
//! (linear BT.709 primaries, 1.0 = 80 nits) becomes BT.2020 / PQ (SMPTE ST 2084) 16-bit RGB,
//! which swscale then turns into 10-bit YUV. This is the CPU path; the GPU path does the same
//! with a shader.

use std::ffi::{c_int, c_uint};
use std::sync::OnceLock;

use ffmpeg_next::ffi;

use rayon::prelude::*;
use vixeeny_common::colour;

/// `src`: `width × height` pixels of RGBA half floats (little endian, rows `stride` bytes
/// apart). Writes into `out` tightly packed 16-bit RGB (`RGB48LE`), PQ-coded, BT.2020 primaries,
/// full range; `None` when `src` is too short.
pub fn scrgb_half_to_pq(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    out: &mut Vec<u8>,
) -> Option<()> {
    let (w, h) = (width as usize, height as usize);
    if stride < w * 8 || src.len() < stride * h.saturating_sub(1) + w * 8 {
        return None;
    }
    // Half → f32 table: one lookup per channel instead of a bit shuffle.
    static HALF: OnceLock<Vec<f32>> = OnceLock::new();
    let half = HALF.get_or_init(|| (0..=u16::MAX).map(vixeeny_common::half::to_f32).collect());
    // One row per task, over every core: a 4K frame is 25 million samples.
    out.resize(w * h * 6, 0);
    out.par_chunks_mut(w * 6)
        .zip(src.par_chunks(stride))
        .for_each(|(out, row)| {
            let pixels = row[..w * 8].as_chunks::<8>().0;
            for (o, px) in out.as_chunks_mut::<6>().0.iter_mut().zip(pixels) {
                let channel = |i: usize| half[usize::from(u16::from_le_bytes([px[i], px[i + 1]]))];
                let codes = colour::scrgb_to_pq([channel(0), channel(2), channel(4)]);
                for (o, code) in o.as_chunks_mut::<2>().0.iter_mut().zip(codes) {
                    *o = code.to_le_bytes();
                }
            }
        });
    Some(())
}

/// `AVMasteringDisplayMetadata` (libavutil/mastering_display_metadata.h).
#[repr(C)]
struct MasteringDisplay {
    display_primaries: [[ffi::AVRational; 2]; 3],
    white_point: [ffi::AVRational; 2],
    min_luminance: ffi::AVRational,
    max_luminance: ffi::AVRational,
    has_primaries: c_int,
    has_luminance: c_int,
}

/// `AVContentLightMetadata`.
#[repr(C)]
struct ContentLight {
    max_cll: c_uint,
    max_fall: c_uint,
}

/// HDR10 static metadata, so players map the picture to their own screen: the mastering display
/// is the recorded monitor (BT.2020 primaries, as the PQ pixels are, a D65 white and its peak
/// brightness), and that peak bounds both the brightest pixel and the brightest frame average
/// (not measured; readers drop a light level whose average is 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hdr10 {
    peak_nits: u32,
}

impl Hdr10 {
    pub(crate) fn new(peak_nits: f32) -> Self {
        Self {
            peak_nits: peak_nits.clamp(80.0, 10_000.0).round() as u32,
        }
    }

    fn mastering(self) -> MasteringDisplay {
        let xy = |x: f64, y: f64| {
            [x, y].map(|v| ffi::AVRational {
                num: (v * 50_000.0).round() as c_int,
                den: 50_000,
            })
        };
        MasteringDisplay {
            // Red, green, blue.
            display_primaries: [xy(0.708, 0.292), xy(0.170, 0.797), xy(0.131, 0.046)],
            white_point: xy(0.3127, 0.3290),
            // The monitor's black is not read: the usual 0.0001 nits.
            min_luminance: ffi::AVRational {
                num: 1,
                den: 10_000,
            },
            max_luminance: ffi::AVRational {
                num: self.peak_nits as c_int,
                den: 1,
            },
            has_primaries: 1,
            has_luminance: 1,
        }
    }

    fn light(self) -> ContentLight {
        ContentLight {
            max_cll: self.peak_nits,
            max_fall: self.peak_nits,
        }
    }

    /// Writes the metadata on an encoder before it opens: x265, SVT-AV1 and NVENC read it there,
    /// and FFmpeg passes it on to the container.
    ///
    /// # Safety
    /// `ctx` is a live encoder context that is not open yet.
    pub(crate) unsafe fn attach_to_context(self, ctx: *mut ffi::AVCodecContext) {
        // SAFETY: per the caller; each entry is allocated at the size of the struct written.
        unsafe {
            let sd = &raw mut (*ctx).decoded_side_data;
            let nb = &raw mut (*ctx).nb_decoded_side_data;
            let add = |kind, size| ffi::av_frame_side_data_new(sd, nb, kind, size, 0);
            write(
                add(MASTERING, size_of::<MasteringDisplay>()),
                self.mastering(),
            );
            write(add(LIGHT, size_of::<ContentLight>()), self.light());
        }
    }

    /// Writes the metadata on a frame that has none: AMF and Quick Sync read it there.
    ///
    /// # Safety
    /// `frame` is a live frame.
    pub(crate) unsafe fn attach_to_frame(self, frame: *mut ffi::AVFrame) {
        // SAFETY: per the caller; each entry is allocated at the size of the struct written.
        unsafe {
            if ffi::av_frame_get_side_data(frame, MASTERING).is_null() {
                write(
                    ffi::av_frame_new_side_data(frame, MASTERING, size_of::<MasteringDisplay>()),
                    self.mastering(),
                );
            }
            if ffi::av_frame_get_side_data(frame, LIGHT).is_null() {
                write(
                    ffi::av_frame_new_side_data(frame, LIGHT, size_of::<ContentLight>()),
                    self.light(),
                );
            }
        }
    }
}

const MASTERING: ffi::AVFrameSideDataType =
    ffi::AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA;
const LIGHT: ffi::AVFrameSideDataType = ffi::AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL;

/// Copies `value` into the side data `sd` (allocated at its size); a failed allocation is left
/// out.
///
/// # Safety
/// `sd` is null or a side data entry of `size_of::<T>()` bytes.
unsafe fn write<T>(sd: *mut ffi::AVFrameSideData, value: T) {
    if sd.is_null() {
        return;
    }
    // SAFETY: per the caller; FFmpeg's buffers are aligned for any type.
    unsafe { (*sd).data.cast::<T>().write(value) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_pq(src: &[u8], width: u32, height: u32, stride: usize) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        scrgb_half_to_pq(src, width, height, stride, &mut out).map(|()| out)
    }

    const ZERO: u16 = 0;
    const ONE: u16 = 0x3C00;
    const HALF: u16 = 0x3800;
    const ONE_QUARTER_UP: u16 = 0x3D00; // 1.25
    const TWO: u16 = 0x4000;

    fn pixel(r: u16, g: u16, b: u16) -> Vec<u8> {
        [r, g, b, ONE]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect()
    }

    fn codes(px: &[u8]) -> Vec<f64> {
        px.as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(u16::from_le_bytes(*c)) / 65535.0)
            .collect()
    }

    #[test]
    fn hdr10_metadata_names_the_monitor_peak_on_bt2020() {
        let meta = Hdr10::new(603.4);
        let m = meta.mastering();
        assert_eq!((m.max_luminance.num, m.max_luminance.den), (603, 1));
        // Green of BT.2020, in steps of 1/50000.
        assert_eq!(m.display_primaries[1][1].num, 39_850);
        assert_eq!(meta.light().max_cll, 603);
        assert_eq!(Hdr10::new(0.0).peak_nits, 80);
    }

    #[test]
    fn white_and_black_keep_their_levels_through_the_matrix() {
        let src = [
            pixel(ZERO, ZERO, ZERO),
            pixel(ONE_QUARTER_UP, ONE_QUARTER_UP, ONE_QUARTER_UP),
        ]
        .concat();
        let out = to_pq(&src, 2, 1, 16).unwrap();
        let c = codes(&out);
        assert_eq!(&c[..3], &[0.0, 0.0, 0.0]);
        // Neutral grey stays neutral (the matrix rows sum to 1): 100 nits.
        for v in &c[3..] {
            assert!((v - 0.5081).abs() < 2e-3, "{v}");
        }
    }

    #[test]
    fn saturated_bt709_red_moves_inside_the_bt2020_gamut() {
        let out = to_pq(&pixel(ONE, ZERO, ZERO), 1, 1, 8).unwrap();
        let c = codes(&out);
        // Red 709 = (0.627, 0.069, 0.016) in linear 2020: green and blue are small but not 0.
        assert!(c[0] > c[1] && c[1] > c[2] && c[2] > 0.0, "{c:?}");
    }

    #[test]
    fn above_the_peak_clips_and_negatives_go_black() {
        // 2.0 is 160 nits: fine. Clipping is exercised through a huge half (65504).
        let mut px = pixel(TWO, ZERO, ZERO);
        px[..2].copy_from_slice(&0x7BFFu16.to_le_bytes());
        px[2..4].copy_from_slice(&0xBC00u16.to_le_bytes()); // −1.0
        let c = codes(&to_pq(&px, 1, 1, 8).unwrap());
        assert!(c[0] <= 1.0 && c[0] > 0.99, "{c:?}");
        assert!(c[1] >= 0.0);
    }

    #[test]
    fn rows_may_be_padded_and_short_buffers_are_refused() {
        let mut src = pixel(ONE, ONE, ONE);
        src.extend([0u8; 8]); // padding up to stride 16
        src.extend(pixel(HALF, HALF, HALF));
        let out = to_pq(&src, 1, 2, 16).unwrap();
        assert_eq!(out.len(), 12);
        assert!(to_pq(&src[..10], 1, 2, 16).is_none());
    }
}
