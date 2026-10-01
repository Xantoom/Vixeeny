// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR → SDR tone mapping (plan 5.2, M5). Input: scRGB, i.e. linear BT.709 floats where `1.0`
//! is 80 nits (what Windows Graphics Capture returns for an HDR monitor in
//! `R16G16B16A16Float`).
//!
//! Algorithm, per pixel:
//! 1. Normalise so that the user's SDR white level (the "SDR content brightness" setting)
//!    becomes `1.0`: `v = scRGB × 80 / sdr_white_nits`. SDR content inside an HDR desktop thus
//!    keeps its look.
//! 2. Compute the luminance `Y` (BT.709) and map it with [`map_luminance`]: identity up to a
//!    knee, then an extended-Reinhard roll-off of the excess so that the display peak lands
//!    exactly on `1.0` (smooth, monotonic, no clipping).
//! 3. Scale RGB by `Y'/Y` (hue and saturation are preserved). If a channel then exceeds `1.0`,
//!    divide all channels by the largest one: a very bright saturated colour becomes a little
//!    darker but keeps its hue and saturation, instead of being clipped or washed out to white.
//! 4. Encode with the sRGB transfer function and quantise to 8 bits.

/// Where the identity segment ends, as a fraction of SDR white.
pub const KNEE: f32 = 0.8;

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

/// Luminance roll-off. `headroom` is the peak in units of SDR white (`peak / sdr_white`).
pub fn map_luminance(y: f32, headroom: f32) -> f32 {
    if y <= KNEE {
        return y.max(0.0);
    }
    let headroom = headroom.max(1.0);
    let x = (y - KNEE) / (1.0 - KNEE);
    // Extended Reinhard on the excess: g(0)=0, g'(0)=1, g(w)=1.
    let w = ((headroom - KNEE) / (1.0 - KNEE)).max(1.0);
    let g = x * (1.0 + x / (w * w)) / (1.0 + x);
    KNEE + (1.0 - KNEE) * g.min(1.0)
}

fn srgb_oetf(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Maps one scRGB pixel to sRGB-encoded `[r, g, b]` in `0.0..=1.0`.
pub fn tonemap_pixel(rgb: [f32; 3], params: &ToneMapParams) -> [f32; 3] {
    let scale = 80.0 / params.sdr_white_nits;
    let headroom = params.peak_nits / params.sdr_white_nits;
    let [r, g, b] = rgb.map(|c| (c * scale).max(0.0));
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if y <= f32::EPSILON {
        return [0.0; 3];
    }
    let y_out = map_luminance(y, headroom);
    let k = y_out / y;
    let mut out = [r * k, g * k, b * k];
    let max = out[0].max(out[1]).max(out[2]);
    if max > 1.0 {
        out = out.map(|c| c / max);
    }
    out.map(srgb_oetf)
}

/// Converts a whole frame: `rgba` has 4 floats per pixel (alpha ignored), the result is
/// 8-bit BGRA with opaque alpha.
pub fn tonemap_frame(rgba: &[f32], params: &ToneMapParams) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        let [r, g, b] = tonemap_pixel([px[0], px[1], px[2]], params);
        let q = |v: f32| (v * 255.0 + 0.5) as u8;
        out.extend_from_slice(&[q(b), q(g), q(r), 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_up_to_the_knee() {
        for y in [0.0, 0.1, 0.5, KNEE] {
            assert!((map_luminance(y, 12.5) - y).abs() < 1e-6);
        }
    }

    #[test]
    fn peak_lands_on_one_without_overshoot() {
        for headroom in [1.5, 4.0, 12.5, 50.0] {
            let top = map_luminance(headroom, headroom);
            assert!((top - 1.0).abs() < 1e-4, "headroom {headroom}: {top}");
            // Anything brighter is simply clamped to 1.
            assert!(map_luminance(headroom * 2.0, headroom) <= 1.0);
        }
    }

    #[test]
    fn monotonic_and_continuous() {
        let mut last = 0.0;
        for i in 0..2000 {
            let y = i as f32 * 0.01;
            let v = map_luminance(y, 12.5);
            assert!(v >= last - 1e-6, "not monotonic at {y}");
            assert!(v - last < 0.02, "jump at {y}");
            last = v;
        }
    }

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

    #[test]
    fn frame_conversion_is_bgra() {
        let out = tonemap_frame(
            &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            &ToneMapParams::default(),
        );
        assert_eq!(out.len(), 8);
        assert!(out[2] > 200 && out[0] == 0 && out[1] == 0 && out[3] == 255);
        assert_eq!(&out[4..], &[0, 0, 0, 255]);
    }
}
