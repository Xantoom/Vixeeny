// SPDX-License-Identifier: GPL-3.0-or-later
//! Pixel effects on a straight 4-channel buffer: Gaussian-like blur and mosaic. They run on the flattened image at
//! export, which is what makes them irreversible.

use rayon::prelude::*;

/// Integer pixel region inside a `width`×`height` buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Region {
    /// Rounds a float rectangle outwards and clamps it to the buffer. `None` when empty.
    pub fn clamp(rect: &crate::Rect, width: usize, height: usize) -> Option<Self> {
        let x0 = rect.x.floor().max(0.0) as usize;
        let y0 = rect.y.floor().max(0.0) as usize;
        let x1 = (rect.right().ceil().max(0.0) as usize).min(width);
        let y1 = (rect.bottom().ceil().max(0.0) as usize).min(height);
        (x1 > x0 && y1 > y0).then(|| Self { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

/// The widest box a pass takes (pixels): the division by a multiplication below is exact up to it.
const MAX_WINDOW: usize = (1 << 16) - 1;

/// One box blur pass along a line of pixels, edges replicated: `dst[i]` is the rounded mean of `src[i - radius..=i +
/// radius]`.
fn box_pass(src: &[[u8; 4]], dst: &mut [[u8; 4]], radius: usize) {
    let Some(last) = src.len().checked_sub(1) else {
        return;
    };
    let window = 2 * radius + 1;
    // `(s + window / 2) / window` as a multiplication and a shift: exact for every sum of `window` bytes below
    // `MAX_WINDOW`, and many times faster than four divisions a pixel.
    let scale = (1_u64 << 40).div_ceil(window as u64);
    let half = (window / 2) as u32;
    let at = |i: usize| src[i.min(last)];
    let mut sum = src[0].map(|v| u32::from(v) * (radius as u32 + 1));
    for i in 1..=radius {
        let p = at(i);
        for c in 0..4 {
            sum[c] += u32::from(p[c]);
        }
    }
    for (i, out) in dst.iter_mut().enumerate() {
        *out = sum.map(|s| ((u64::from(s + half) * scale) >> 40) as u8);
        let (add, sub) = (at(i + radius + 1), src[i.saturating_sub(radius)]);
        for c in 0..4 {
            sum[c] = sum[c] + u32::from(add[c]) - u32::from(sub[c]);
        }
    }
}

/// Three box blurs along each of `rows` rows of `n` pixels (`stride` bytes apart, from the start of `data`), the rows
/// shared between threads: each row stays in the cache for its three passes.
fn box_rows(data: &mut [u8], stride: usize, rows: usize, n: usize, radius: usize) {
    if rows == 0 || n == 0 {
        return;
    }
    let Some(data) = data.get_mut(..(rows - 1) * stride + n * 4) else {
        return;
    };
    let blur_rows = |chunk: &mut [u8]| {
        let (mut once, mut twice) = (vec![[0; 4]; n], vec![[0; 4]; n]);
        for row in chunk.chunks_mut(stride) {
            let row = row[..n * 4].as_chunks_mut::<4>().0;
            box_pass(row, &mut once, radius);
            box_pass(&once, &mut twice, radius);
            box_pass(&twice, row, radius);
        }
    };
    // Small selections need no scheduling; the large ones reuse workers across both axes and successive blurs.
    if rows * n < 1 << 16 {
        blur_rows(data);
    } else {
        let per = rows.div_ceil(rayon::current_num_threads().min(8));
        data.par_chunks_mut(per * stride).for_each(blur_rows);
    }
}

/// Pixels per side of the tiles a transposition copies, so both sides stay in the cache.
const TILE: usize = 32;

/// Copies the `w`×`h` pixels of `src` (`src_stride` bytes per row) to `dst` with rows and columns swapped (`dst_stride`
/// bytes per row of `h` pixels). Bands of rows of `dst` are shared between threads.
fn transpose(src: &[u8], src_stride: usize, dst: &mut [u8], dst_stride: usize, (w, h): (usize, usize)) {
    let band = |(t, rows): (usize, &mut [u8])| {
        let x0 = t * TILE;
        for ty in (0..h).step_by(TILE) {
            for x in x0..(x0 + TILE).min(w) {
                let row = &mut rows[(x - x0) * dst_stride..];
                for y in ty..(ty + TILE).min(h) {
                    row[y * 4..y * 4 + 4].copy_from_slice(&src[y * src_stride + x * 4..][..4]);
                }
            }
        }
    };
    let (size, bands) = (dst_stride * TILE, w.div_ceil(TILE));
    if w * h < 1 << 16 {
        dst.chunks_mut(size).take(bands).enumerate().for_each(band);
    } else {
        dst.par_chunks_mut(size).take(bands).enumerate().for_each(band);
    }
}

/// Approximates a Gaussian blur of standard deviation `sigma` by three box blurs. The columns are blurred as the rows
/// of a transposed copy: walking down a column of a large image misses the cache at every pixel.
pub fn blur(data: &mut [u8], width: usize, region: Region, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    let w_ideal = (4.0 * sigma * sigma + 1.0).sqrt();
    let radius = (((w_ideal - 1.0) / 2.0).round() as usize).clamp(1, MAX_WINDOW / 2 - 1);
    let stride = width * 4;
    let Some(area) = data.get_mut((region.y * width + region.x) * 4..) else {
        return;
    };
    let (w, h) = (region.w, region.h);
    box_rows(area, stride, h, w, radius);
    let mut columns = vec![0; w * h * 4];
    transpose(area, stride, &mut columns, h * 4, (w, h));
    box_rows(&mut columns, h * 4, w, h, radius);
    transpose(&columns, h * 4, area, stride, (h, w));
}

/// Replaces every `block`×`block` cell of the region (aligned to the region's corner) by its average colour.
pub fn pixelate(data: &mut [u8], width: usize, region: Region, block: usize) {
    let block = block.max(1);
    for by in (0..region.h).step_by(block) {
        for bx in (0..region.w).step_by(block) {
            let (bw, bh) = (block.min(region.w - bx), block.min(region.h - by));
            let mut sum = [0u32; 4];
            for y in 0..bh {
                for x in 0..bw {
                    let o = ((region.y + by + y) * width + region.x + bx + x) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(data[o + c]);
                    }
                }
            }
            let count = (bw * bh) as u32;
            let avg = sum.map(|s| ((s + count / 2) / count) as u8);
            for y in 0..bh {
                for x in 0..bw {
                    let o = ((region.y + by + y) * width + region.x + bx + x) * 4;
                    data[o..o + 4].copy_from_slice(&avg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One box blur pass along a line of `n` pixels, `stride` bytes apart, with edge replication, as first written.
    fn box_line(data: &mut [u8], start: usize, stride: usize, n: usize, radius: usize, tmp: &mut Vec<[u8; 4]>) {
        tmp.clear();
        tmp.extend((0..n).map(|i| {
            let o = start + i * stride;
            [data[o], data[o + 1], data[o + 2], data[o + 3]]
        }));
        let window = (2 * radius + 1) as u32;
        let at = |i: isize| tmp[i.clamp(0, n as isize - 1) as usize];
        let mut sum = [0u32; 4];
        for i in -(radius as isize)..=(radius as isize) {
            let p = at(i);
            for c in 0..4 {
                sum[c] += u32::from(p[c]);
            }
        }
        for i in 0..n {
            let o = start + i * stride;
            for c in 0..4 {
                data[o + c] = ((sum[c] + window / 2) / window) as u8;
            }
            let (add, sub) = (at(i as isize + radius as isize + 1), at(i as isize - radius as isize));
            for c in 0..4 {
                sum[c] = sum[c] + u32::from(add[c]) - u32::from(sub[c]);
            }
        }
    }

    #[test]
    fn a_pass_is_the_line_by_line_one() {
        let mut seed = 0x2545_f491_u32;
        let mut byte = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        };
        for n in [1, 2, 3, 7, 40, 301] {
            for radius in [1, 2, 3, 5, 12, 50, 400] {
                let line: Vec<u8> = (0..n * 4).map(|_| byte()).collect();
                let mut want = line.clone();
                box_line(&mut want, 0, 4, n, radius, &mut Vec::new());
                let mut got = vec![[0; 4]; n];
                box_pass(line.as_chunks::<4>().0, &mut got, radius);
                assert_eq!(got.as_flattened(), want, "{n} pixels, radius {radius}");
            }
        }
        // The widest window, all white: still exact.
        let white = vec![[255; 4]; 3];
        let mut got = vec![[0; 4]; 3];
        box_pass(&white, &mut got, MAX_WINDOW / 2);
        assert_eq!(got, white);
    }

    #[test]
    fn the_division_by_a_multiplication_is_exact_for_every_sum() {
        for window in [3_u32, 5, 151, 4095, MAX_WINDOW as u32] {
            let scale = (1_u64 << 40).div_ceil(u64::from(window));
            let step = if window > 5000 { 97 } else { 1 };
            for s in (0..=255 * window).step_by(step).chain([255 * window]) {
                let s = s + window / 2;
                assert_eq!((u64::from(s) * scale) >> 40, u64::from(s / window), "{s} / {window}");
            }
        }
    }

    /// The blur as it was first written: rows then columns, three times, one line at a time.
    fn reference(data: &mut [u8], width: usize, region: Region, radius: usize) {
        let mut tmp = Vec::new();
        for _ in 0..3 {
            for row in 0..region.h {
                let start = ((region.y + row) * width + region.x) * 4;
                box_line(data, start, 4, region.w, radius, &mut tmp);
            }
            for col in 0..region.w {
                let start = (region.y * width + region.x + col) * 4;
                box_line(data, start, width * 4, region.h, radius, &mut tmp);
            }
        }
    }

    #[test]
    fn the_blur_matches_the_line_by_line_one_and_stays_in_its_region() {
        // Large enough for several threads; the region off every edge.
        let (width, height) = (420, 380);
        let image: Vec<u8> = (0..width * height * 4).map(|i| (i * 7919 % 251) as u8).collect();
        let region = Region { x: 13, y: 7, w: 397, h: 361 };
        let sigma = 4.0_f32;
        let radius = ((((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).round() as usize).max(1);
        let mut want = image.clone();
        reference(&mut want, width, region, radius);
        let mut got = image.clone();
        blur(&mut got, width, region, sigma);
        for (i, ((g, w), o)) in got.iter().zip(&want).zip(&image).enumerate() {
            let (x, y) = (i / 4 % width, i / 4 / width);
            let inside = (region.x..region.x + region.w).contains(&x) && (region.y..region.y + region.h).contains(&y);
            if inside {
                // Only the rounding between passes differs.
                assert!(g.abs_diff(*w) <= 2, "pixel ({x}, {y}): {g} vs {w}");
            } else {
                assert_eq!(g, o, "pixel ({x}, {y}) is outside the region");
            }
        }
    }
}
