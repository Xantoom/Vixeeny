// SPDX-License-Identifier: GPL-3.0-or-later
//! A captured window set on a background: a margin around it, its corners rounded as Windows 11
//! draws them, a soft shadow under it, over a blur of the window itself or a plain colour.
//! Worked out in linear light, so that the same code serves SDR pictures (linear 0…1) and HDR
//! ones (scRGB); the blur and the shadow blend as light does.

use rayon::prelude::*;
use vixeeny_common::colour::srgb_decode;

/// What goes behind the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Backdrop {
    /// The window itself, enlarged and heavily blurred.
    Blur,
    /// A plain sRGB colour.
    Colour([u8; 3]),
}

/// A picture in linear light, 4 floats per pixel (alpha ignored on input, 1 on output).
#[derive(Debug, Clone, PartialEq)]
pub struct Linear {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
}

/// Margin and corner radius at 100 %, in pixels; they grow with the screen's scale.
const MARGIN: f32 = 48.0;
const RADIUS: f32 = 8.0;
/// How much the blur backdrop is darkened (so the window stands out), and how much the shadow
/// darkens at its heart.
pub const DARKEN: f32 = 0.8;
pub const SHADOW: f32 = 0.45;

/// The sizes of a window set on its background, in pixels (what the graphics card draws too).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The whole picture.
    pub size: (u32, u32),
    pub margin: u32,
    pub radius: f32,
    /// How far down the shadow is, and the radius of its three box blurs.
    pub offset: u32,
    pub softness: u32,
    /// The blur backdrop is worked out at this size, then blurred by three box blurs of
    /// `small_radius`.
    pub small: (u32, u32),
    pub small_radius: u32,
}

/// The layout of a `width`×`height` window at the interface `scale` of its screen.
pub fn layout(width: u32, height: u32, scale: f32) -> Layout {
    let margin = (MARGIN * scale).round().max(8.0) as u32;
    let (w, h) = (width + 2 * margin, height + 2 * margin);
    // Worked out small (an eighth), then smoothly enlarged: the same look, far less work.
    let small = ((w / 8).max(4), (h / 8).max(4));
    Layout {
        size: (w, h),
        margin,
        radius: RADIUS * scale,
        offset: (margin as f32 * 0.25).round() as u32,
        softness: (margin as f32 * 0.4).round().max(1.0) as u32,
        small,
        small_radius: (small.0.max(small.1) / 12).max(2),
    }
}

/// A plain backdrop colour in linear light, at `white` (see [`set_on`]).
pub fn linear_colour(c: [u8; 3], white: f32) -> [f32; 3] {
    c.map(|v| srgb_decode(f32::from(v) / 255.0) * white)
}

/// `window` on `backdrop`, at the interface `scale` of its screen. `white`: SDR white in the
/// picture's units (1.0 for SDR, the screen's SDR white / 80 for scRGB), the brightness of the
/// colour backdrop.
pub fn set_on(window: &Linear, backdrop: Backdrop, scale: f32, white: f32) -> Linear {
    let layout = layout(window.width, window.height, scale);
    let Layout {
        size: (w, h),
        margin,
        radius,
        ..
    } = layout;
    let mut out = match backdrop {
        Backdrop::Colour(c) => {
            let px = linear_colour(c, white);
            Linear {
                width: w,
                height: h,
                rgba: [px[0], px[1], px[2], 1.0].repeat(w as usize * h as usize),
            }
        }
        Backdrop::Blur => blurred(window, &layout),
    };
    // The shadow: the window's shape, lower, softened, darkening what is under it.
    let shape = rounded(window.width, window.height, radius);
    let offset = layout.offset as usize;
    let softness = layout.softness as usize;
    let mut shadow = vec![0.0_f32; w as usize * h as usize];
    for y in 0..window.height as usize {
        let dst = (y + margin as usize + offset) * w as usize + margin as usize;
        if y + margin as usize + offset >= h as usize {
            break;
        }
        let src = y * window.width as usize;
        shadow[dst..dst + window.width as usize]
            .copy_from_slice(&shape[src..src + window.width as usize]);
    }
    blur_plane(&mut shadow, w as usize, h as usize, softness);
    out.rgba
        .par_chunks_mut(4)
        .zip(shadow.par_iter())
        .for_each(|(px, &s)| {
            let keep = 1.0 - SHADOW * s;
            px[0] *= keep;
            px[1] *= keep;
            px[2] *= keep;
        });
    // The window, its corners cut with anti-aliasing.
    let row = w as usize * 4;
    out.rgba
        .par_chunks_mut(row)
        .skip(margin as usize)
        .take(window.height as usize)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row =
                &window.rgba[y * window.width as usize * 4..][..window.width as usize * 4];
            for x in 0..window.width as usize {
                let a = shape[y * window.width as usize + x];
                if a <= 0.0 {
                    continue;
                }
                let o = &mut out_row[(margin as usize + x) * 4..][..4];
                for c in 0..3 {
                    o[c] = o[c] * (1.0 - a) + src_row[x * 4 + c] * a;
                }
                o[3] = 1.0;
            }
        });
    out
}

/// Coverage of a `w`×`h` rectangle with corners of `radius`: 1 inside, 0 outside, a pixel's
/// share along the curve.
fn rounded(w: u32, h: u32, radius: f32) -> Vec<f32> {
    let r = radius.min(w as f32 / 2.0).min(h as f32 / 2.0);
    let mut out = vec![1.0_f32; w as usize * h as usize];
    if r <= 0.0 {
        return out;
    }
    let reach = r.ceil() as u32;
    for y in 0..reach.min(h) {
        for x in 0..reach.min(w) {
            // Distance from the corner circle's centre, at the pixel's centre.
            let (dx, dy) = (r - (x as f32 + 0.5), r - (y as f32 + 0.5));
            let a = (r + 0.5 - (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt()).clamp(0.0, 1.0);
            for (cx, cy) in [
                (x, y),
                (w - 1 - x, y),
                (x, h - 1 - y),
                (w - 1 - x, h - 1 - y),
            ] {
                out[(cy * w + cx) as usize] = a;
            }
        }
    }
    out
}

/// The window stretched over `w`×`h` and blurred well past recognition, a little darker so the
/// window stands out.
fn blurred(window: &Linear, layout: &Layout) -> Linear {
    let (w, h) = layout.size;
    let (sw, sh) = (layout.small.0 as usize, layout.small.1 as usize);
    let (ww, wh) = (window.width as usize, window.height as usize);
    let mut small = vec![[0.0_f32; 3]; sw * sh];
    for (i, px) in small.iter_mut().enumerate() {
        let (x, y) = (i % sw, i / sw);
        // The mean of the window pixels this small pixel covers.
        let (x0, x1) = (x * ww / sw, ((x + 1) * ww / sw).max(x * ww / sw + 1));
        let (y0, y1) = (y * wh / sh, ((y + 1) * wh / sh).max(y * wh / sh + 1));
        let mut sum = [0.0_f32; 3];
        for yy in y0..y1.min(wh) {
            for xx in x0..x1.min(ww) {
                let p = &window.rgba[(yy * ww + xx) * 4..];
                sum[0] += p[0];
                sum[1] += p[1];
                sum[2] += p[2];
            }
        }
        let n = ((y1.min(wh) - y0) * (x1.min(ww) - x0)).max(1) as f32;
        *px = sum.map(|s| s / n * DARKEN);
    }
    let radius = layout.small_radius as usize;
    for c in 0..3 {
        let mut plane: Vec<f32> = small.iter().map(|p| p[c]).collect();
        blur_plane(&mut plane, sw, sh, radius);
        for (p, v) in small.iter_mut().zip(plane) {
            p[c] = v;
        }
    }
    // Bilinear enlargement to the full size.
    let mut rgba = vec![0.0_f32; w as usize * h as usize * 4];
    rgba.par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(y, row)| {
            let fy = ((y as f32 + 0.5) * sh as f32 / h as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(sh - 1);
            for x in 0..w as usize {
                let fx =
                    ((x as f32 + 0.5) * sw as f32 / w as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(sw - 1);
                for c in 0..3 {
                    let top = small[y0 * sw + x0][c] * (1.0 - tx) + small[y0 * sw + x1][c] * tx;
                    let bottom = small[y1 * sw + x0][c] * (1.0 - tx) + small[y1 * sw + x1][c] * tx;
                    row[x * 4 + c] = top * (1.0 - ty) + bottom * ty;
                }
                row[x * 4 + 3] = 1.0;
            }
        });
    Linear {
        width: w,
        height: h,
        rgba,
    }
}

/// Three box blurs of `radius` in each direction: close to a Gaussian. Edges repeat.
fn blur_plane(plane: &mut [f32], w: usize, h: usize, radius: usize) {
    let mut line = Vec::new();
    for _ in 0..3 {
        for y in 0..h {
            box_line(&mut plane[y * w..(y + 1) * w], radius, &mut line);
        }
        let mut column = vec![0.0; h];
        for x in 0..w {
            for y in 0..h {
                column[y] = plane[y * w + x];
            }
            box_line(&mut column, radius, &mut line);
            for y in 0..h {
                plane[y * w + x] = column[y];
            }
        }
    }
}

/// One box blur along `values`, by a running sum.
fn box_line(values: &mut [f32], radius: usize, scratch: &mut Vec<f32>) {
    let n = values.len();
    if n == 0 || radius == 0 {
        return;
    }
    scratch.clear();
    scratch.extend_from_slice(values);
    let at = |i: isize| scratch[i.clamp(0, n as isize - 1) as usize];
    let r = radius as isize;
    let mut sum: f32 = (-r..=r).map(at).sum();
    let width = (2 * radius + 1) as f32;
    for (i, v) in values.iter_mut().enumerate() {
        *v = sum / width;
        let i = i as isize;
        sum += at(i + r + 1) - at(i - r);
    }
}

/// SDR BGRA → linear light.
pub fn linear_from_bgra(width: u32, height: u32, stride: usize, bgra: &[u8]) -> Linear {
    let lut: Vec<f32> = (0..=255).map(|v| srgb_decode(v as f32 / 255.0)).collect();
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        for px in bgra[y * stride..][..width as usize * 4].as_chunks::<4>().0 {
            rgba.extend_from_slice(&[
                lut[usize::from(px[2])],
                lut[usize::from(px[1])],
                lut[usize::from(px[0])],
                1.0,
            ]);
        }
    }
    Linear {
        width,
        height,
        rgba,
    }
}

/// Linear light → SDR BGRA, opaque.
pub fn linear_to_bgra(picture: &Linear) -> Vec<u8> {
    // 4096 steps: finer than 8 bits everywhere on the sRGB curve.
    let lut: Vec<u8> = (0..4096)
        .map(|i| {
            let v = i as f32 / 4095.0;
            let s = if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round() as u8
        })
        .collect();
    let code = |v: f32| lut[(v.clamp(0.0, 1.0) * 4095.0).round() as usize];
    picture
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [code(p[2]), code(p[1]), code(p[0]), 255])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(w: u32, h: u32, v: f32) -> Linear {
        Linear {
            width: w,
            height: h,
            rgba: [v, v, v, 1.0].repeat((w * h) as usize),
        }
    }

    #[test]
    fn the_window_sits_in_a_margin_with_rounded_corners() {
        let out = set_on(
            &window(200, 100, 0.5),
            Backdrop::Colour([255, 255, 255]),
            1.0,
            1.0,
        );
        assert_eq!((out.width, out.height), (296, 196));
        let px = |x: u32, y: u32| out.rgba[((y * out.width + x) * 4) as usize];
        // The middle of the window is the window.
        assert!((px(148, 98) - 0.5).abs() < 1e-6);
        // Far from it, the colour as it is; the shadow darkens under the window.
        assert!((px(2, 2) - 1.0).abs() < 1e-3);
        assert!(px(148, 100 + 48 + 8) < 0.9);
        // The very corner of the window shows the background (and its shadow), not the window.
        assert!(px(48, 48) > 0.6);
    }

    #[test]
    fn hdr_backdrops_are_at_sdr_white() {
        // scRGB on a screen whose SDR white is 240 nits: 3.0.
        let out = set_on(
            &window(64, 64, 10.0),
            Backdrop::Colour([255, 255, 255]),
            1.0,
            3.0,
        );
        assert!((out.rgba[0] - 3.0).abs() < 1e-3);
        let centre = ((out.height / 2 * out.width + out.width / 2) * 4) as usize;
        assert!((out.rgba[centre] - 10.0).abs() < 1e-4);
    }

    #[test]
    fn a_blur_backdrop_takes_the_window_colours() {
        let mut w = window(160, 90, 0.0);
        for px in w.rgba.as_chunks_mut::<4>().0 {
            px[0] = 0.8; // red
        }
        let out = set_on(&w, Backdrop::Blur, 1.5, 1.0);
        assert_eq!((out.width, out.height), (160 + 144, 90 + 144));
        let corner = &out.rgba[..4];
        assert!(
            corner[0] > 0.4 && corner[1] < 0.05 && corner[2] < 0.05,
            "{corner:?}"
        );
    }

    #[test]
    fn corners_are_smooth() {
        let shape = rounded(40, 40, 8.0);
        assert_eq!(shape[0], 0.0);
        assert_eq!(shape[20 * 40 + 20], 1.0);
        assert_eq!(shape[20], 1.0);
        // Along the curve, partial coverage.
        assert!(shape.iter().any(|&a| a > 0.0 && a < 1.0));
        // Symmetric.
        assert_eq!(shape[40 + 2], shape[2 * 40 + 1]);
        assert_eq!(shape[3], shape[36]);
    }

    #[test]
    fn sdr_round_trips_through_linear_light() {
        let bgra: Vec<u8> = (0..=255u8).flat_map(|v| [v, 255 - v, v / 2, 255]).collect();
        let back = linear_to_bgra(&linear_from_bgra(256, 1, 256 * 4, &bgra));
        assert_eq!(back, bgra);
    }

    #[test]
    fn box_blur_keeps_the_mean() {
        let mut plane: Vec<f32> = (0..100)
            .map(|i| if i == 50 { 100.0 } else { 0.0 })
            .collect();
        blur_plane(&mut plane, 100, 1, 3);
        assert!((plane.iter().sum::<f32>() - 100.0).abs() < 1e-3);
        assert!(plane[50] < 30.0 && plane[47] > 0.0);
    }
}
