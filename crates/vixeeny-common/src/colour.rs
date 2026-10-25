// SPDX-License-Identifier: GPL-3.0-or-later
//! Colour for HDR, shared by the screenshots and the recordings.
//!
//! HDR → SDR (`vixeeny_image::tonemap` from scRGB, the preview of the video editor from PQ): linear light where `1.0`
//! is SDR white, BT.709 primaries. HDR kept as HDR: scRGB to BT.2020 / PQ ([`scrgb_to_pq`]).

use std::sync::OnceLock;

/// Where the identity segment of [`roll_off`] ends, as a fraction of SDR white.
pub const KNEE: f32 = 0.8;

/// Luminance of linear BT.709 RGB.
pub fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Highlights roll off instead of clipping: identity up to [`KNEE`], then an extended Reinhard on the excess so that
/// `headroom` (the peak, in units of SDR white) lands exactly on `1.0`. Smooth and monotonic.
pub fn roll_off(v: f32, headroom: f32) -> f32 {
    if v <= KNEE {
        return v.max(0.0);
    }
    let span = 1.0 - KNEE;
    let x = (v - KNEE) / span;
    // g(0) = 0, g'(0) = 1, g(w) = 1.
    let w = ((headroom.max(1.0) - KNEE) / span).max(1.0);
    let g = x * (1.0 + x / (w * w)) / (1.0 + x);
    KNEE + span * g.min(1.0)
}

/// The headroom (in units of SDR white) a picture is tone mapped with: its brightest luminance (see [`brightness`]),
/// bounded by SDR white and by `display`, the screen's own. With nothing brighter than SDR white, [`roll_off`] is the
/// identity up to it: SDR content (a page, a window) comes out exactly as an SDR capture shows it; highlights are
/// rolled off only as far as needed.
pub fn headroom(brightest: f32, display: f32) -> f32 {
    let brightest = if brightest.is_finite() { brightest } else { 0.0 };
    brightest.min(display).max(1.0)
}

/// How fast a video's tone mapping follows its brightest pixel, in seconds: quickly when it rises (a flash keeps its
/// highlights), slowly when it falls (no visible pumping).
pub const FOLLOW_RISE_SECONDS: f64 = 0.15;
pub const FOLLOW_FALL_SECONDS: f64 = 1.0;

/// The share of the way to its target a followed value makes at each frame of `fps`, rising and falling.
pub fn follow_steps(fps: f64) -> (f32, f32) {
    let step = |seconds: f64| (1.0 - (-1.0 / (fps.max(1.0) * seconds)).exp()) as f32;
    (step(FOLLOW_RISE_SECONDS), step(FOLLOW_FALL_SECONDS))
}

/// The headroom followed one frame on, towards `target`; `current` below 1 (none yet: a first frame, a seek) takes the
/// target as it is.
pub fn follow(current: f32, target: f32, (rise, fall): (f32, f32)) -> f32 {
    if current >= 1.0 { current + (target - current) * if target > current { rise } else { fall } } else { target }
}

/// The luminance [`roll_off`] maps for one scRGB pixel scaled by `scale` (SDR white becomes `1.0`): what [`headroom`]
/// takes the brightest of. Not a number counts as black.
pub fn brightness(rgb: [f32; 3], scale: f32) -> f32 {
    let y = luminance(into_gamut(rgb.map(|c| c * scale)));
    if y.is_finite() { y } else { 0.0 }
}

/// Brings a colour outside the BT.709 gamut (a negative channel: a wider-gamut green, a deep cyan…) inside it by
/// desaturating it towards the grey of the same luminance, just enough. Hue and brightness stay; clipping the channel
/// instead would shift the hue and add light.
pub fn into_gamut(rgb: [f32; 3]) -> [f32; 3] {
    let y = luminance(rgb);
    if y <= 0.0 {
        return [0.0; 3];
    }
    let low = rgb[0].min(rgb[1]).min(rgb[2]);
    if low >= 0.0 {
        return rgb;
    }
    let t = y / (y - low);
    rgb.map(|c| (y + (c - y) * t).max(0.0))
}

/// The sRGB transfer function, `0.0..=1.0` in and out.
pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// The inverse of [`srgb_encode`].
pub fn srgb_decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// The tone mapper and the PQ coding of this module in HLSL, line for line, for the shaders that run them on the
/// graphics card (stills and recordings): the same steps and the same numbers.
pub const HLSL: &str = r"
static const float KNEE = 0.8;

float luminance(float3 c) { return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b; }

float3 into_gamut(float3 c) {
    float y = luminance(c);
    if (y <= 0.0) return 0.0;
    float low = min(c.r, min(c.g, c.b));
    if (low >= 0.0) return c;
    float t = y / (y - low);
    return max(y + (c - y) * t, 0.0);
}

float roll_off(float v, float h) {
    if (v <= KNEE) return max(v, 0.0);
    float span = 1.0 - KNEE;
    float x = (v - KNEE) / span;
    float w = max((max(h, 1.0) - KNEE) / span, 1.0);
    float g = x * (1.0 + x / (w * w)) / (1.0 + x);
    return KNEE + span * min(g, 1.0);
}

// Not a number, or infinite, counts as black.
float brightness(float3 c, float scale) {
    float y = luminance(into_gamut(c * scale));
    return y > 0.0 && y < 3.0e38 ? y : 0.0;
}

float headroom(float brightest, float display) {
    float b = brightest < 3.0e38 ? brightest : 0.0;
    return max(min(b, display), 1.0);
}

float srgb(float v) {
    v = saturate(v);
    return v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

// One scRGB pixel scaled by `scale` to sRGB-coded 0..1, rolled off to `h`.
float3 tonemap(float3 c, float scale, float h) {
    c = into_gamut(c * scale);
    float y = luminance(c);
    if (!(y > 1.1920929e-7)) return 0.0;
    float3 o = c * (roll_off(y, h) / y);
    float m = max(o.r, max(o.g, o.b));
    if (m > 1.0) o /= m;
    return float3(srgb(o.r), srgb(o.g), srgb(o.b));
}

// Linear BT.709 to linear BT.2020 (ITU-R BT.2087).
static const float3x3 BT709_TO_2020 = {
    0.627404, 0.329283, 0.043313,
    0.069097, 0.919540, 0.011362,
    0.016391, 0.088013, 0.895595,
};

// PQ inverse EOTF of `light` (1.0 = 10 000 nits).
float pq(float light) {
    const float m1 = 2610.0 / 16384.0;
    const float m2 = 2523.0 / 4096.0 * 128.0;
    const float c1 = 3424.0 / 4096.0;
    const float c2 = 2413.0 / 4096.0 * 32.0;
    const float c3 = 2392.0 / 4096.0 * 32.0;
    float y = pow(saturate(light), m1);
    return pow((c1 + c2 * y) / (1.0 + c3 * y), m2);
}

// One scRGB pixel as BT.2020 / PQ signals (0..1). Not a number counts as black.
float3 scrgb_to_pq(float3 c) {
    float3 l = mul(BT709_TO_2020, c) * (80.0 / 10000.0);
    l = l < 3.0e38 ? l : 0.0;
    return float3(pq(l.r), pq(l.g), pq(l.b));
}
";

// ---- HDR: scRGB to BT.2020 / PQ (SMPTE ST 2084), for the recordings and the HDR screenshots.

/// The brightest light PQ codes, in nits.
pub const PQ_PEAK_NITS: f32 = 10_000.0;

/// The same in scRGB units (1.0 = 80 nits).
const PQ_PEAK: f32 = PQ_PEAK_NITS / 80.0;

/// Linear BT.709 → linear BT.2020 (ITU-R BT.2087).
const BT709_TO_2020: [[f32; 3]; 3] =
    [[0.627_404, 0.329_283, 0.043_313], [0.069_097, 0.919_540, 0.011_362], [0.016_391, 0.088_013, 0.895_595]];

/// Linear BT.2020 → linear BT.709 (the inverse of [`BT709_TO_2020`]).
const BT2020_TO_709: [[f32; 3]; 3] =
    [[1.660_491, -0.587_641, -0.072_850], [-0.124_550, 1.132_9, -0.008_349], [-0.018_151, -0.100_579, 1.118_73]];

/// PQ inverse EOTF: absolute luminance (0…10 000 nits, as 0…1) → signal (0…1).
fn pq_encode(linear: f64) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let y = linear.clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

/// PQ EOTF: signal (0…1) → absolute luminance (0…10 000 nits, as 0…1).
fn pq_decode(signal: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 4096.0 * 128.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 4096.0 * 32.0;
    const C3: f32 = 2392.0 / 4096.0 * 32.0;
    let p = signal.clamp(0.0, 1.0).powf(1.0 / M2);
    ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

/// Smallest value the table distinguishes from black (2⁻²⁰ ≈ 1e-6 of SDR white).
const FLOOR_BITS: u32 = (127 - 20) << 23;

/// Linear scRGB value → PQ code (0…65535), through a table indexed by the top bits of the float (10 mantissa bits: a
/// relative step of 0.1 %, far below one 10-bit code).
struct Table {
    codes: Vec<u16>,
}

impl Table {
    fn build() -> Self {
        let last = Self::key(PQ_PEAK);
        let codes = (0..=last)
            .map(|k| {
                let bits = ((k + (FLOOR_BITS >> 13)) << 13) | 0x1000; // bucket centre
                let nits = f64::from(f32::from_bits(bits)) * 80.0;
                (pq_encode(nits / 10_000.0) * 65535.0).round() as u16
            })
            .collect();
        Self { codes }
    }

    fn key(x: f32) -> u32 {
        (x.to_bits() >> 13) - (FLOOR_BITS >> 13)
    }

    fn code(&self, x: f32) -> u16 {
        if x.is_nan() || x < f32::from_bits(FLOOR_BITS) {
            return 0;
        }
        let x = x.min(PQ_PEAK);
        self.codes[Self::key(x) as usize]
    }
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::build)
}

/// One scRGB pixel (linear BT.709, 1.0 = 80 nits) as BT.2020 / PQ codes, full range 16 bits. Colours wider than BT.709
/// (negative channels) land inside BT.2020; above 10 000 nits clips.
pub fn scrgb_to_pq(rgb: [f32; 3]) -> [u16; 3] {
    let table = table();
    scrgb_to_bt2020(rgb).map(|c| table.code(c))
}

/// One scRGB pixel on BT.2020 primaries, still linear and in the same units.
pub fn scrgb_to_bt2020(rgb: [f32; 3]) -> [f32; 3] {
    BT709_TO_2020.map(|m| m[0] * rgb[0] + m[1] * rgb[1] + m[2] * rgb[2])
}

/// The inverse of [`scrgb_to_pq`]: BT.2020 / PQ signals (0…1) → scRGB. Colours outside BT.709 come out with negative
/// channels, as scRGB writes them.
pub fn pq_to_scrgb(signal: [f32; 3]) -> [f32; 3] {
    let linear = signal.map(|s| pq_decode(s) * PQ_PEAK);
    BT2020_TO_709.map(|m| m[0] * linear[0] + m[1] * linear[1] + m[2] * linear[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_wider_than_srgb_keeps_its_hue_and_brightness() {
        // A deep BT.2020 cyan, as BT.709: red goes negative.
        let cyan = [-0.66, 1.12, 1.02];
        let fitted = into_gamut(cyan);
        assert!(fitted.iter().all(|&c| c >= 0.0), "{fitted:?}");
        assert!((luminance(fitted) - luminance(cyan)).abs() < 1e-5);
        // Still a cyan leaning green: green > blue > red.
        assert!(fitted[1] > fitted[2] && fitted[2] > fitted[0], "{fitted:?}");
        // Clipping would have brightened it.
        assert!(luminance([0.0, 1.12, 1.02]) > luminance(fitted) + 0.1);
        // Colours already inside are untouched.
        assert_eq!(into_gamut([0.2, 0.5, 0.1]), [0.2, 0.5, 0.1]);
    }

    #[test]
    fn a_picture_without_highlights_keeps_sdr_white() {
        // Nothing brighter than SDR white: white stays white, a light grey stays itself.
        let h = headroom(1.0, 1000.0 / 240.0);
        assert_eq!(roll_off(1.0, h), 1.0);
        assert!((roll_off(0.9, h) - 0.9).abs() < 1e-6);
        // Highlights: rolled off only up to the brightest one, never past the screen's peak.
        assert_eq!(headroom(2.5, 1000.0 / 240.0), 2.5);
        assert_eq!(headroom(40.0, 4.0), 4.0);
        assert_eq!(headroom(f32::NAN, 4.0), 1.0);
        assert_eq!(brightness([f32::INFINITY, 0.0, 0.0], 1.0), 0.0);
    }

    #[test]
    fn highlights_roll_off_to_the_peak_without_overshoot() {
        let headroom = 1000.0 / 203.0;
        assert_eq!(roll_off(0.5, headroom), 0.5);
        assert!((roll_off(headroom, headroom) - 1.0).abs() < 1e-5);
        let mut last = 0.0;
        for i in 0..=200 {
            let v = roll_off(i as f32 / 20.0, headroom);
            assert!(v >= last && v <= 1.0);
            last = v;
        }
    }

    #[test]
    fn pq_matches_the_reference_points() {
        assert!(pq_encode(0.0) < 1e-5);
        assert!((pq_encode(1.0) - 1.0).abs() < 1e-12);
        // 100 nits is 0.508 on the PQ scale (BT.2100 table).
        assert!((pq_encode(100.0 / 10_000.0) - 0.5081).abs() < 5e-4);
        // 1000 nits: 0.7518.
        assert!((pq_encode(0.1) - 0.7518).abs() < 5e-4);
    }

    #[test]
    fn pq_round_trips_through_scrgb() {
        for rgb in [[1.0, 1.0, 1.0], [12.5, 3.0, 0.2], [0.01, 0.5, 40.0], [-0.3, 1.1, 1.0]] {
            let codes = scrgb_to_pq(rgb).map(|c| f32::from(c) / 65535.0);
            let back = pq_to_scrgb(codes);
            // The matrices mix the channels: the error follows the brightest one.
            let max = rgb.iter().fold(0.0_f32, |m, c| m.max(c.abs()));
            for (a, b) in back.iter().zip(rgb) {
                assert!((a - b).abs() < 0.002 * max, "{rgb:?} → {back:?}");
            }
        }
    }
}
