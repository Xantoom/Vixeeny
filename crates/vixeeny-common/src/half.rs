// SPDX-License-Identifier: GPL-3.0-or-later
//! IEEE 754 half-precision floats, as stored by Windows for HDR screens (scRGB).

/// Half → single precision (exact: every half is a float).
pub fn to_f32(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = u32::from((h >> 10) & 0x1f);
    let mant = u32::from(h & 0x3ff);
    match exp {
        // Subnormal: mant × 2⁻²⁴.
        0 => {
            let v = mant as f32 * (1.0 / 16_777_216.0);
            if sign == 0 { v } else { -v }
        }
        0x1f => f32::from_bits(sign | 0x7f80_0000 | (mant << 13)),
        _ => f32::from_bits(sign | ((exp + 112) << 23) | (mant << 13)),
    }
}

/// Single → half precision, rounded to nearest; too large becomes infinite.
pub fn from_f32(v: f32) -> u16 {
    let sign = ((v.to_bits() >> 16) & 0x8000) as u16;
    if v.is_nan() {
        return sign | 0x7e00;
    }
    let a = v.abs();
    if a >= 65_520.0 {
        return sign | 0x7c00;
    }
    let bits = a.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let man = bits & 0x7f_ffff;
    let magnitude = if exp <= 0 {
        // Subnormal half (or zero).
        if exp < -10 {
            0
        } else {
            let m = man | 0x80_0000;
            let shift = (14 - exp) as u32;
            (m >> shift) + ((m >> (shift - 1)) & 1)
        }
    } else {
        // A rounding carry correctly moves into the exponent.
        ((exp as u32) << 10 | man >> 13) + ((man >> 12) & 1)
    };
    sign | magnitude as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_convert_both_ways() {
        assert_eq!(to_f32(0x3c00), 1.0);
        assert_eq!(to_f32(0xc000), -2.0);
        assert_eq!(to_f32(0x0000), 0.0);
        assert_eq!(to_f32(0x3555), 0.333_251_95);
        assert_eq!(to_f32(0x7bff), 65504.0);
        assert!(to_f32(0x7c00).is_infinite());
        assert!(to_f32(0x7e00).is_nan());
        assert_eq!(to_f32(0x0001), 2f32.powi(-24)); // smallest subnormal
        assert_eq!(to_f32(0x8001), -(2f32.powi(-24)));
        for h in [0x0000, 0x0001, 0x03ff, 0x0400, 0x3555, 0x3c00, 0x7bff, 0x8001, 0xc000] {
            assert_eq!(from_f32(to_f32(h)), h, "{h:#06x}");
        }
        assert_eq!(from_f32(0.333_333), 0x3555);
        assert_eq!(from_f32(1e6), 0x7c00);
        assert!(to_f32(from_f32(f32::NAN)).is_nan());
    }
}
