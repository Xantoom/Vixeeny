// SPDX-License-Identifier: GPL-3.0-or-later
//! Tray icon pixels, drawn in code (no image decoder in the daemon): a violet rounded square
//! with a white play triangle; while recording, a red dot in the top-right corner.

pub const SIZE: u32 = 32;

const VIOLET: [u8; 3] = [0x7c, 0x5c, 0xff];
const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const RED: [u8; 3] = [0xe5, 0x39, 0x35];

/// Coverage of a shape at pixel `(x, y)`, from 4×4 supersampling.
fn coverage(x: u32, y: u32, inside: impl Fn(f32, f32) -> bool) -> f32 {
    let mut hits = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let fx = x as f32 + (sx as f32 + 0.5) / 4.0;
            let fy = y as f32 + (sy as f32 + 0.5) / 4.0;
            if inside(fx, fy) {
                hits += 1;
            }
        }
    }
    hits as f32 / 16.0
}

fn in_rounded_square(x: f32, y: f32) -> bool {
    let (min, max, radius) = (1.0, SIZE as f32 - 1.0, 7.0);
    if x < min || x > max || y < min || y > max {
        return false;
    }
    let cx = x.clamp(min + radius, max - radius);
    let cy = y.clamp(min + radius, max - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

fn in_triangle(x: f32, y: f32) -> bool {
    // Play triangle pointing right, slightly right of centre.
    let (ax, ay, bx, by, cx, cy) = (12.0, 8.0, 12.0, 24.0, 25.0, 16.0);
    let side = |px: f32, py: f32, qx: f32, qy: f32| (qx - px) * (y - py) - (qy - py) * (x - px);
    let d1 = side(ax, ay, bx, by);
    let d2 = side(bx, by, cx, cy);
    let d3 = side(cx, cy, ax, ay);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

fn in_dot(x: f32, y: f32) -> bool {
    (x - 24.5).powi(2) + (y - 7.5).powi(2) <= 6.5 * 6.5
}

/// Straight-alpha RGBA, `SIZE × SIZE`.
pub fn render(recording: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let base = coverage(x, y, in_rounded_square);
            let mut rgb = VIOLET.map(f32::from);
            let tri = coverage(x, y, in_triangle);
            for (c, w) in rgb.iter_mut().zip(WHITE) {
                *c += (f32::from(w) - *c) * tri;
            }
            let mut alpha = base;
            if recording {
                let dot = coverage(x, y, in_dot);
                for (c, r) in rgb.iter_mut().zip(RED) {
                    *c += (f32::from(r) - *c) * dot;
                }
                alpha = alpha.max(dot);
            }
            out.extend(rgb.map(|c| c.round() as u8));
            out.push((alpha * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn size_and_corners() {
        let img = render(false);
        assert_eq!(img.len(), (SIZE * SIZE * 4) as usize);
        assert_eq!(
            px(&img, 0, 0)[3],
            0,
            "outside the rounded square is transparent"
        );
        assert_eq!(px(&img, 6, 24)[3], 255, "inside is opaque");
    }

    #[test]
    fn triangle_is_white_and_background_violet() {
        let img = render(false);
        assert_eq!(px(&img, 15, 16), [255, 255, 255, 255]);
        assert_eq!(px(&img, 4, 16), [0x7c, 0x5c, 0xff, 255]);
    }

    #[test]
    fn recording_adds_a_red_dot() {
        let idle = render(false);
        let rec = render(true);
        assert_ne!(idle, rec);
        assert_eq!(px(&rec, 24, 7), [0xe5, 0x39, 0x35, 255]);
        assert_eq!(px(&idle, 24, 7), [0x7c, 0x5c, 0xff, 255]);
    }
}
