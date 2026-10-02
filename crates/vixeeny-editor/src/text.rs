// SPDX-License-Identifier: GPL-3.0-or-later
//! The bundled font (Inter, SIL OFL 1.1, see `assets/fonts`) and text layout shared by the
//! model (bounds, hit-testing) and the renderer.

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

static FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// The editor font. Parsed once.
pub fn font() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(|| {
        // The bytes are a compile-time constant known to parse; the fallback keeps this total.
        Font::from_bytes(FONT_BYTES, FontSettings::default())
            .unwrap_or_else(|e| unreachable!("bundled font is invalid: {e}"))
    })
}

/// One positioned glyph, relative to the text origin (top-left of the first line).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub ch: char,
    pub x: f32,
    /// Baseline y.
    pub baseline: f32,
}

/// Layout of `text` at `size` pixels: glyph positions and the total width/height. Lines are
/// split on `\n`; line height is 1.25 × size.
pub fn layout(text: &str, size: f32) -> (Vec<Glyph>, f32, f32) {
    let font = font();
    let line_height = size * 1.25;
    let ascent = font
        .horizontal_line_metrics(size)
        .map_or(size * 0.95, |m| m.ascent);
    let mut glyphs = Vec::new();
    let mut width = 0f32;
    let mut lines = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        lines = i + 1;
        let mut x = 0f32;
        for ch in line.chars() {
            glyphs.push(Glyph {
                ch,
                x,
                baseline: i as f32 * line_height + ascent,
            });
            x += font.metrics(ch, size).advance_width;
        }
        width = width.max(x);
    }
    (glyphs, width, lines.max(1) as f32 * line_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_loads_and_measures() {
        let (glyphs, w, h) = layout("Hello", 20.0);
        assert_eq!(glyphs.len(), 5);
        assert!(w > 30.0 && w < 80.0, "{w}");
        assert_eq!(h, 25.0);
    }

    #[test]
    fn lines_stack() {
        let (glyphs, w, h) = layout("ab\ncd", 10.0);
        assert_eq!(h, 25.0);
        assert!(glyphs[2].baseline > glyphs[0].baseline);
        assert_eq!(glyphs[2].x, 0.0);
        assert!(w > 0.0);
    }

    #[test]
    fn empty_text_is_one_empty_line() {
        let (glyphs, w, h) = layout("", 10.0);
        assert!(glyphs.is_empty());
        assert_eq!((w, h), (0.0, 12.5));
    }
}
