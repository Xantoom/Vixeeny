// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR → SDR tone mapping. Input: scRGB, i.e. linear BT.709 floats where `1.0`
//! is 80 nits (what Windows Graphics Capture returns for an HDR monitor in
//! `R16G16B16A16Float`).
//!
//! Algorithm, per pixel:
//! 1. Normalise so that the user's SDR white level (the "SDR content brightness" setting)
//!    becomes `1.0`: `v = scRGB × 80 / sdr_white_nits`. SDR content inside an HDR desktop thus
//!    keeps its look.
//! 2. Bring colours wider than sRGB (negative channels) inside it, keeping hue and brightness
//!    ([`into_gamut`]).
//! 3. Compute the luminance `Y` (BT.709) and map it with [`roll_off`]: identity up to a
//!    knee, then an extended-Reinhard roll-off of the excess so that the brightest pixel of the
//!    picture (no brighter than the display's peak) lands exactly on `1.0` (smooth, monotonic,
//!    no clipping). A picture with nothing above SDR white is left as it is: white stays 255.
//! 4. Scale RGB by `Y'/Y` (hue and saturation are preserved). If a channel then exceeds `1.0`,
//!    divide all channels by the largest one: a very bright saturated colour becomes a little
//!    darker but keeps its hue and saturation, instead of being clipped or washed out to white.
//! 5. Encode with the sRGB transfer function and quantise to 8 bits.

use vixeeny_common::colour::{brightness, headroom, into_gamut, luminance, roll_off, srgb_encode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneMapParams {
    /// Brightness of SDR white on the HDR display, in nits (Windows default 80).
    pub sdr_white_nits: f32,
    /// Peak brightness of the display, in nits.
    pub peak_nits: f32,
}

impl Default for ToneMapParams {
    fn default() -> Self {
        Self {
            sdr_white_nits: 80.0,
            peak_nits: 1000.0,
        }
    }
}

/// Maps one scRGB pixel to sRGB-encoded `[r, g, b]` in `0.0..=1.0`, rolled off up to the
/// display's peak (a whole picture is rolled off up to its own brightest pixel: see
/// [`tonemap_frame`]).
pub fn tonemap_pixel(rgb: [f32; 3], params: &ToneMapParams) -> [f32; 3] {
    let scale = 80.0 / params.sdr_white_nits;
    map(rgb, scale, params.peak_nits / params.sdr_white_nits)
}

/// One pixel scaled by `scale` (SDR white becomes 1), rolled off up to `headroom`.
fn map(rgb: [f32; 3], scale: f32, headroom: f32) -> [f32; 3] {
    let [r, g, b] = into_gamut(rgb.map(|c| c * scale));
    let y = luminance([r, g, b]);
    if y <= f32::EPSILON {
        return [0.0; 3];
    }
    let k = roll_off(y, headroom) / y;
    let mut out = [r * k, g * k, b * k];
    let max = out[0].max(out[1]).max(out[2]);
    if max > 1.0 {
        out = out.map(|c| c / max);
    }
    out.map(srgb_encode)
}

/// Converts a whole frame: `rgba` has 4 floats per pixel (alpha ignored), the result is
/// 8-bit BGRA with opaque alpha, rolled off up to the frame's brightest pixel (see
/// [`headroom`]). Spread over every core: a 4K frame takes 200 ms on one.
pub fn tonemap_frame(rgba: &[f32], params: &ToneMapParams) -> Vec<u8> {
    use rayon::prelude::*;

    let scale = 80.0 / params.sdr_white_nits;
    let brightest = rgba
        .par_chunks(4 * 4096)
        .map(|chunk| {
            chunk
                .as_chunks::<4>()
                .0
                .iter()
                .map(|px| brightness([px[0], px[1], px[2]], scale))
                .fold(0.0_f32, f32::max)
        })
        .reduce(|| 0.0, f32::max);
    let headroom = headroom(brightest, params.peak_nits / params.sdr_white_nits);
    let mut out = vec![0; rgba.len()];
    out.par_chunks_mut(4 * 4096)
        .zip(rgba.par_chunks(4 * 4096))
        .for_each(|(out, rgba)| {
            let pixels = out
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(rgba.as_chunks::<4>().0);
            for (o, px) in pixels {
                let [r, g, b] = map([px[0], px[1], px[2]], scale, headroom);
                let q = |v: f32| (v * 255.0 + 0.5) as u8;
                *o = [q(b), q(g), q(r), 255];
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdr_white_stays_close_to_white() {
        // 1.0 (paper white) must not look washed out: at least 240/255.
        let [r, g, b] = tonemap_pixel([1.0; 3], &ToneMapParams::default());
        assert!(r > 240.0 / 255.0 && (r - g).abs() < 1e-6 && (g - b).abs() < 1e-6);
    }

    #[test]
    fn black_and_negatives_are_black() {
        assert_eq!(tonemap_pixel([0.0; 3], &ToneMapParams::default()), [0.0; 3]);
        assert_eq!(
            tonemap_pixel([-0.5, -1.0, 0.0], &ToneMapParams::default()),
            [0.0; 3]
        );
    }

    #[test]
    fn saturated_highlights_keep_their_hue() {
        // A very bright, saturated orange: no channel exceeds 1 and the channel order is kept.
        let [r, g, b] = tonemap_pixel([10.0, 4.0, 0.0], &ToneMapParams::default());
        assert!(r <= 1.0 && r > g && g >= b && r - b > 0.3, "{r} {g} {b}");
        // A moderately bright one keeps visible saturation.
        let [r, g, b] = tonemap_pixel([2.0, 0.8, 0.0], &ToneMapParams::default());
        assert!(r - b > 0.3, "{r} {g} {b}");
    }

    #[test]
    fn a_frame_without_highlights_is_an_sdr_capture() {
        // SDR white at 240 nits on a 1000-nit screen: a white page and a light grey.
        let params = ToneMapParams {
            sdr_white_nits: 240.0,
            peak_nits: 1000.0,
        };
        let white = 240.0 / 80.0;
        let grey = white * 0.8;
        let out = tonemap_frame(&[white, white, white, 1.0, grey, grey, grey, 1.0], &params);
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
        let srgb = (srgb_encode(0.8) * 255.0 + 0.5) as u8;
        assert_eq!(&out[4..7], &[srgb; 3]);
        // A highlight in the frame: white is rolled off a little, the highlight lands on 255.
        let out = tonemap_frame(&[white, white, white, 1.0, 12.5, 12.5, 12.5, 1.0], &params);
        assert!(out[0] < 255 && out[0] > 230, "{}", out[0]);
        assert_eq!(&out[4..7], &[255; 3]);
    }

    #[test]
    fn grey_ramp_stays_neutral_and_ordered() {
        let p = ToneMapParams::default();
        let mut last = 0.0;
        for i in 1..100 {
            let v = i as f32 * 0.15;
            let [r, g, b] = tonemap_pixel([v; 3], &p);
            assert!((r - g).abs() < 1e-5 && (g - b).abs() < 1e-5);
            assert!(r >= last - 1e-6);
            last = r;
        }
    }
}
