// SPDX-License-Identifier: GPL-3.0-or-later
//! Downscaling a capture by a percentage before it is saved. Each pixel of the result is the average of the source area
//! it covers (fractional pixels weighed by their share), so text and thin lines keep their weight and no pixel is
//! skipped. SDR pictures are averaged as stored (sRGB), as browsers and image viewers scale; HDR ones are linear light.

use rayon::prelude::*;

use crate::Bgra;

/// The size of a `width` × `height` picture at `percent` %, never below one pixel.
pub fn scaled_size(width: u32, height: u32, percent: u8) -> (u32, u32) {
    let side = |n: u32| ((u64::from(n) * u64::from(percent) + 50) / 100).clamp(1, u64::from(n)) as u32;
    (side(width), side(height))
}

/// Whether `percent` makes a `width` × `height` picture smaller (100 % and more leave it as it is).
pub fn shrinks(width: u32, height: u32, percent: u8) -> bool {
    percent < 100 && scaled_size(width, height, percent) != (width, height)
}

/// One output pixel's sources along an axis: the first source index and the weight of each one from there (they add up
/// to 1).
struct Taps {
    start: usize,
    weights: Vec<f32>,
}

fn taps(source: u32, target: u32) -> Vec<Taps> {
    let step = f64::from(source) / f64::from(target);
    (0..target)
        .map(|i| {
            let (from, to) = (f64::from(i) * step, (f64::from(i) + 1.0) * step);
            let start = from.floor() as usize;
            let end = (to.ceil() as usize).min(source as usize);
            let weights = (start..end).map(|j| ((to.min(j as f64 + 1.0) - from.max(j as f64)) / step) as f32).collect();
            Taps { start, weights }
        })
        .collect()
}

/// Area-averages `rows` rows of `width` pixels (`channels` values each, row `y` read by `row`) down to `out_w` ×
/// `out_h`, writing each output row through `write`. Output rows are made in parallel, each from one source row's worth
/// of buffer.
fn area<'a, T: Sync + Send + Copy + Default + 'a>(
    (width, height): (u32, u32),
    (out_w, out_h): (u32, u32),
    channels: usize,
    row: impl Fn(usize) -> &'a [T] + Sync,
    load: impl Fn(T) -> f32 + Sync,
    store: impl Fn(f32) -> T + Sync,
) -> Vec<T> {
    let (across, down) = (taps(width, out_w), taps(height, out_h));
    let out_row = out_w as usize * channels;
    let mut out = vec![T::default(); out_row * out_h as usize];
    out.par_chunks_mut(out_row).zip(down.par_iter()).for_each(|(dst, tap)| {
        // The source rows of this output row, blended into one.
        let mut line = vec![0.0f32; width as usize * channels];
        for (k, w) in tap.weights.iter().enumerate() {
            for (acc, v) in line.iter_mut().zip(row(tap.start + k)) {
                *acc += load(*v) * w;
            }
        }
        for (pixel, tap) in dst.chunks_exact_mut(channels).zip(&across) {
            for (c, out) in pixel.iter_mut().enumerate() {
                let sum: f32 =
                    tap.weights.iter().enumerate().map(|(k, w)| line[(tap.start + k) * channels + c] * w).sum();
                *out = store(sum);
            }
        }
    });
    out
}

/// `image` at `percent` %, packed (stride = width × 4); `None` when that does not make it smaller.
pub fn downscale_bgra(image: &Bgra<'_>, percent: u8) -> Option<(u32, u32, Vec<u8>)> {
    if !shrinks(image.width, image.height, percent) {
        return None;
    }
    let (w, h) = scaled_size(image.width, image.height, percent);
    let row_len = image.width as usize * 4;
    let (data, stride) = (image.data, image.stride);
    let pixels = area(
        (image.width, image.height),
        (w, h),
        4,
        |y| &data[y * stride..y * stride + row_len],
        f32::from,
        |v| v.round().clamp(0.0, 255.0) as u8,
    );
    Some((w, h, pixels))
}

/// Linear RGBA (scRGB floats) at `percent` %; `None` when that does not make it smaller.
pub fn downscale_rgba_f32(width: u32, height: u32, rgba: &[f32], percent: u8) -> Option<(u32, u32, Vec<f32>)> {
    if !shrinks(width, height, percent) {
        return None;
    }
    let (w, h) = scaled_size(width, height, percent);
    let row_len = width as usize * 4;
    let pixels = area((width, height), (w, h), 4, |y| &rgba[y * row_len..(y + 1) * row_len], |v| v, |v| v);
    Some((w, h, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_and_never_reach_zero() {
        assert_eq!(scaled_size(1920, 1080, 50), (960, 540));
        assert_eq!(scaled_size(1920, 1080, 75), (1440, 810));
        assert_eq!(scaled_size(1920, 1080, 25), (480, 270));
        assert_eq!(scaled_size(3, 1, 25), (1, 1));
        assert_eq!(scaled_size(100, 100, 100), (100, 100));
        assert!(!shrinks(100, 100, 100));
        assert!(!shrinks(1, 1, 50));
        assert!(shrinks(2, 2, 50));
    }

    #[test]
    fn half_size_averages_each_square_of_four() {
        // 2 × 2 pixels: black, white, white, black → one mid grey; alpha stays opaque.
        let data = [0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255];
        let (w, h, out) = downscale_bgra(&Bgra::new(2, 2, 8, &data), 50).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, [128, 128, 128, 255]);
    }

    #[test]
    fn three_quarters_weighs_partly_covered_pixels() {
        // One row of 4 pixels (0, 100, 200, 40) → 3: each covers 4/3 source pixels.
        let data: Vec<u8> = [0u8, 100, 200, 40].iter().flat_map(|&v| [v, v, v, 255]).collect();
        let (w, h, out) = downscale_bgra(&Bgra::new(4, 1, 16, &data), 75).unwrap();
        assert_eq!((w, h), (3, 1));
        let grey: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
        // 0·¾ + 100·¼ = 25; 100·½ + 200·½ = 150; 200·¼ + 40·¾ = 80.
        assert_eq!(grey, [25, 150, 80]);
    }

    #[test]
    fn a_padded_stride_is_skipped() {
        // Rows of 2 pixels padded to 12 bytes with junk.
        let mut data = vec![9u8; 24];
        data[..8].copy_from_slice(&[10, 20, 30, 255, 30, 40, 50, 255]);
        data[12..20].copy_from_slice(&[10, 20, 30, 255, 30, 40, 50, 255]);
        let (_, _, out) = downscale_bgra(&Bgra::new(2, 2, 12, &data), 50).unwrap();
        assert_eq!(out, [20, 30, 40, 255]);
    }

    #[test]
    fn a_flat_picture_stays_flat() {
        let data = vec![77u8; 1000 * 7 * 4];
        let (w, h, out) = downscale_bgra(&Bgra::new(1000, 7, 4000, &data), 25).unwrap();
        assert_eq!((w, h), (250, 2));
        assert!(out.iter().all(|&v| v == 77));
    }

    #[test]
    fn hdr_is_averaged_in_linear_light() {
        let rgba = [0.0, 0.0, 0.0, 1.0, 4.0, 4.0, 4.0, 1.0];
        let (w, h, out) = downscale_rgba_f32(2, 1, &rgba, 50).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, [2.0, 2.0, 2.0, 1.0]);
    }

    #[test]
    fn full_size_is_left_alone() {
        let data = [1u8; 16];
        assert!(downscale_bgra(&Bgra::new(2, 2, 8, &data), 100).is_none());
    }
}
