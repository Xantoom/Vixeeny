// SPDX-License-Identifier: GPL-3.0-or-later
//! The colours of the editor: the design tokens of `vixeeny-ui/ui/theme.slint`, for the look the
//! host gives (light or dark, the system accent).

/// Straight-alpha RGBA, 0.0–1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub [f32; 4]);

impl Rgba {
    pub const fn hex(rgb: u32, alpha: u8) -> Self {
        Self([
            ((rgb >> 16) & 0xff) as f32 / 255.0,
            ((rgb >> 8) & 0xff) as f32 / 255.0,
            (rgb & 0xff) as f32 / 255.0,
            alpha as f32 / 255.0,
        ])
    }

    pub fn rgb8(c: [u8; 3]) -> Self {
        Self([
            f32::from(c[0]) / 255.0,
            f32::from(c[1]) / 255.0,
            f32::from(c[2]) / 255.0,
            1.0,
        ])
    }

    pub fn with_alpha(self, a: f32) -> Self {
        let [r, g, b, _] = self.0;
        Self([r, g, b, a])
    }

    /// Slint's `brighter` (value × (1 + f)) and `darker` (value ÷ (1 + f)), in HSV.
    fn scale_value(self, factor: f32) -> Self {
        let [r, g, b, a] = self.0;
        let (h, s, v) = rgb_to_hsv(r, g, b);
        let (r, g, b) = hsv_to_rgb(h, s, (v * factor).clamp(0.0, 1.0));
        Self([r, g, b, a])
    }

    pub fn brighter(self, f: f32) -> Self {
        self.scale_value(1.0 + f)
    }

    pub fn darker(self, f: f32) -> Self {
        self.scale_value(1.0 / (1.0 + f))
    }
}

/// `h` in degrees (0–360), `s` and `v` 0–1.
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}

/// How the host wants the editor to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    /// The system accent colour.
    pub accent: [u8; 3],
    /// Off when the user turned animations off in Windows.
    pub animations: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            dark: true,
            accent: [0x00, 0x78, 0xd4],
            animations: true,
        }
    }
}

/// The colours the editor draws with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub flyout: Rgba,
    pub stroke_strong: Rgba,
    pub divider: Rgba,
    pub text: Rgba,
    pub text_off: Rgba,
    pub subtle_hover: Rgba,
    pub subtle_pressed: Rgba,
    pub accent: Rgba,
    pub accent_soft: Rgba,
    pub animations: bool,
}

impl Theme {
    pub fn new(look: Look) -> Self {
        let dark = look.dark;
        let base = Rgba::rgb8(look.accent);
        let accent = if dark {
            base.brighter(0.45)
        } else {
            base.darker(0.05)
        };
        let pick = |d: Rgba, l: Rgba| if dark { d } else { l };
        Self {
            flyout: pick(Rgba::hex(0x232327, 255), Rgba::hex(0xffffff, 255)),
            stroke_strong: pick(Rgba::hex(0xffffff, 0x24), Rgba::hex(0x000000, 0x24)),
            divider: pick(Rgba::hex(0xffffff, 0x0b), Rgba::hex(0x000000, 0x0b)),
            text: pick(Rgba::hex(0xededef, 255), Rgba::hex(0x18181b, 255)),
            text_off: pick(Rgba::hex(0xffffff, 0x4d), Rgba::hex(0x000000, 0x40)),
            subtle_hover: pick(Rgba::hex(0xffffff, 0x0d), Rgba::hex(0x000000, 0x0a)),
            subtle_pressed: pick(Rgba::hex(0xffffff, 0x08), Rgba::hex(0x000000, 0x10)),
            accent,
            accent_soft: accent.with_alpha(0.16),
            animations: look.animations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips() {
        for (r, g, b) in [
            (1.0, 0.0, 0.0),
            (0.2, 0.4, 0.6),
            (0.9, 0.9, 0.1),
            (0.0, 0.0, 0.0),
        ] {
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (r2, g2, b2) = hsv_to_rgb(h, s, v);
            assert!((r - r2).abs() < 1e-5 && (g - g2).abs() < 1e-5 && (b - b2).abs() < 1e-5);
        }
    }

    #[test]
    fn the_dark_accent_is_brighter() {
        let theme = Theme::new(Look::default());
        // #0078d4 has value 0.83: ×1.45 clamps to 1.0.
        let (_, _, v) = rgb_to_hsv(theme.accent.0[0], theme.accent.0[1], theme.accent.0[2]);
        assert!((v - 1.0).abs() < 1e-5);
    }
}
