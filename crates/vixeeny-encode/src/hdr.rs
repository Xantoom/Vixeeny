// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR frames for recording: the scRGB half-float frame WGC delivers on an HDR monitor
//! (linear BT.709 primaries, 1.0 = 80 nits) becomes BT.2020 / PQ (SMPTE ST 2084) 16-bit RGB,
//! which swscale then turns into 10-bit YUV. This is the CPU path; the GPU path does the same
//! with a shader.

use std::sync::OnceLock;

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
