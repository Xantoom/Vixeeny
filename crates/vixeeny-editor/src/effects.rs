// SPDX-License-Identifier: GPL-3.0-or-later
//! Pixel effects on a straight 4-channel buffer: Gaussian-like blur and mosaic. They run on the
//! flattened image at export, which is what makes them irreversible.

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
        (x1 > x0 && y1 > y0).then(|| Self {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// One box blur pass along a line of `n` pixels, `stride` bytes apart, with edge replication.
fn box_line(
    data: &mut [u8],
    start: usize,
    stride: usize,
    n: usize,
    radius: usize,
    tmp: &mut Vec<[u8; 4]>,
) {
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
        let (add, sub) = (
            at(i as isize + radius as isize + 1),
            at(i as isize - radius as isize),
        );
        for c in 0..4 {
            sum[c] = sum[c] + u32::from(add[c]) - u32::from(sub[c]);
        }
    }
}

/// Approximates a Gaussian blur of standard deviation `sigma` by three box blurs.
pub fn blur(data: &mut [u8], width: usize, region: Region, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    let w_ideal = (4.0 * sigma * sigma + 1.0).sqrt();
    let radius = (((w_ideal - 1.0) / 2.0).round() as usize).max(1);
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

/// Replaces every `block`×`block` cell of the region (aligned to the region's corner) by its
/// average colour.
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
