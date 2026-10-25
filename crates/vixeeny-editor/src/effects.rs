// SPDX-License-Identifier: GPL-3.0-or-later
//! Pixel effects on a straight 4-channel buffer: Gaussian-like blur and mosaic. They run on the flattened image at
//! export, which is what makes them irreversible.

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

/// One box blur pass along a line of `n` pixels, `stride` bytes apart, with edge replication.
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

/// Three box blurs along each of `rows` rows of `n` pixels (`stride` bytes apart, from the start of `data`), the rows
/// shared between threads: each row stays in the cache for its three passes.
fn box_rows(data: &mut [u8], stride: usize, rows: usize, n: usize, radius: usize) {
    if rows == 0 || n == 0 {
        return;
    }
    let Some(data) = data.get_mut(..(rows - 1) * stride + n * 4) else {
        return;
    };
    // Below this many pixels, starting threads costs more than it saves.
    let threads = if rows * n < 1 << 16 {
        1
    } else {
        std::thread::available_parallelism().map_or(1, std::num::NonZero::get).min(8).min(rows)
    };
    let per = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        for chunk in data.chunks_mut(per * stride) {
            scope.spawn(move || {
                let mut tmp = Vec::with_capacity(n);
                for row in chunk.chunks_mut(stride) {
                    for _ in 0..3 {
                        box_line(row, 0, 4, n, radius, &mut tmp);
                    }
                }
            });
        }
    });
}

/// Pixels per side of the tiles a transposition copies, so both sides stay in the cache.
const TILE: usize = 32;

/// Copies the `w`×`h` pixels of `src` (`src_stride` bytes per row) to `dst` with rows and columns swapped (`dst_stride`
/// bytes per row of `h` pixels).
fn transpose(src: &[u8], src_stride: usize, dst: &mut [u8], dst_stride: usize, (w, h): (usize, usize)) {
    for ty in (0..h).step_by(TILE) {
        for tx in (0..w).step_by(TILE) {
            for y in ty..(ty + TILE).min(h) {
                for x in tx..(tx + TILE).min(w) {
                    let (s, d) = (y * src_stride + x * 4, x * dst_stride + y * 4);
                    dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
        }
    }
}

/// Approximates a Gaussian blur of standard deviation `sigma` by three box blurs. The columns are blurred as the rows
/// of a transposed copy: walking down a column of a large image misses the cache at every pixel.
pub fn blur(data: &mut [u8], width: usize, region: Region, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    let w_ideal = (4.0 * sigma * sigma + 1.0).sqrt();
    let radius = (((w_ideal - 1.0) / 2.0).round() as usize).max(1);
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
