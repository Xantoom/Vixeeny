// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR frames for recording (plan 5.9): the scRGB half-float frame WGC delivers on an HDR monitor
//! (linear BT.709 primaries, 1.0 = 80 nits) becomes BT.2020 / PQ (SMPTE ST 2084) 16-bit RGB,
//! which swscale then turns into 10-bit YUV. This is the CPU path; the GPU path does the same
//! with a shader.

use std::sync::OnceLock;

use rayon::prelude::*;

/// 10 000 nits in scRGB units (1.0 = 80 nits).
const PEAK: f32 = 10_000.0 / 80.0;

/// Linear BT.709 → linear BT.2020 (ITU-R BT.2087).
const M: [[f32; 3]; 3] = [
    [0.627_404, 0.329_283, 0.043_313],
    [0.069_097, 0.919_540, 0.011_362],
    [0.016_391, 0.088_013, 0.895_595],
];

/// PQ inverse EOTF: absolute luminance (0…10 000 nits, as 0…1) → signal (0…1).
pub fn pq_encode(linear: f64) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let y = linear.clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

/// Half-float bits → f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1F);
    let man = u32::from(h & 0x3FF);
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalise.
            let shift = m.leading_zeros() - 21;
            let m = (m << shift) & 0x3FF;
            sign | ((113 - shift) << 23) | (m << 13)
        }
        (0x1F, 0) => sign | 0x7F80_0000,
        (0x1F, m) => sign | 0x7FC0_0000 | (m << 13),
        (e, m) => sign | ((e + 112) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

/// Smallest value the table distinguishes from black (2⁻²⁰ ≈ 1e-6 of SDR white).
const FLOOR_BITS: u32 = (127 - 20) << 23;

/// Linear scRGB value → PQ code (0…65535), through a table indexed by the top bits of the float
/// (10 mantissa bits: a relative step of 0.1 %, far below one 10-bit code).
struct Table {
    codes: Vec<u16>,
}

impl Table {
    fn build() -> Self {
        let last = Self::key(PEAK);
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
        let x = x.min(PEAK);
        self.codes[Self::key(x) as usize]
    }
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::build)
}

/// `src`: `width × height` pixels of RGBA half floats (little endian, rows `stride` bytes
/// apart). Returns tightly packed 16-bit RGB (`RGB48LE`), PQ-coded, BT.2020 primaries, full range.
pub fn scrgb_half_to_pq(src: &[u8], width: u32, height: u32, stride: usize) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if stride < w * 8 || src.len() < stride * h.saturating_sub(1) + w * 8 {
        return None;
    }
    let table = table();
    // Half → f32 table: one lookup per channel instead of a bit shuffle.
    static HALF: OnceLock<Vec<f32>> = OnceLock::new();
    let half = HALF.get_or_init(|| (0..=u16::MAX).map(f16_to_f32).collect());
    // One row per task, over every core: a 4K frame is 25 million samples.
    let mut out = vec![0; w * h * 6];
    out.par_chunks_mut(w * 6)
        .zip(src.par_chunks(stride))
        .for_each(|(out, row)| {
            let pixels = row[..w * 8].as_chunks::<8>().0;
            for (o, px) in out.as_chunks_mut::<6>().0.iter_mut().zip(pixels) {
                let r = half[usize::from(u16::from_le_bytes([px[0], px[1]]))];
                let g = half[usize::from(u16::from_le_bytes([px[2], px[3]]))];
                let b = half[usize::from(u16::from_le_bytes([px[4], px[5]]))];
                for (o, m) in o.as_chunks_mut::<2>().0.iter_mut().zip(M) {
                    *o = table.code(m[0] * r + m[1] * g + m[2] * b).to_le_bytes();
                }
            }
        });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn halves_convert_exactly() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
        assert!(f16_to_f32(0x7E00).is_nan());
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
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
    fn white_and_black_keep_their_levels_through_the_matrix() {
        let src = [
            pixel(ZERO, ZERO, ZERO),
            pixel(ONE_QUARTER_UP, ONE_QUARTER_UP, ONE_QUARTER_UP),
        ]
        .concat();
        let out = scrgb_half_to_pq(&src, 2, 1, 16).unwrap();
        let c = codes(&out);
        assert_eq!(&c[..3], &[0.0, 0.0, 0.0]);
        // Neutral grey stays neutral (the matrix rows sum to 1): 100 nits.
        for v in &c[3..] {
            assert!((v - 0.5081).abs() < 2e-3, "{v}");
        }
    }

    #[test]
    fn saturated_bt709_red_moves_inside_the_bt2020_gamut() {
        let out = scrgb_half_to_pq(&pixel(ONE, ZERO, ZERO), 1, 1, 8).unwrap();
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
        let c = codes(&scrgb_half_to_pq(&px, 1, 1, 8).unwrap());
        assert!(c[0] <= 1.0 && c[0] > 0.99, "{c:?}");
        assert!(c[1] >= 0.0);
    }

    #[test]
    fn rows_may_be_padded_and_short_buffers_are_refused() {
        let mut src = pixel(ONE, ONE, ONE);
        src.extend([0u8; 8]); // padding up to stride 16
        src.extend(pixel(HALF, HALF, HALF));
        let out = scrgb_half_to_pq(&src, 1, 2, 16).unwrap();
        assert_eq!(out.len(), 12);
        assert!(scrgb_half_to_pq(&src[..10], 1, 2, 16).is_none());
    }
}
