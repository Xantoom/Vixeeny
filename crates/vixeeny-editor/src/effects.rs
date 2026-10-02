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

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: usize, h: usize) -> Vec<u8> {
        let mut d = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y) % 2 == 0 { 0 } else { 255 };
                let o = (y * w + x) * 4;
                d[o..o + 3].fill(v);
            }
        }
        d
    }

    #[test]
    fn region_clamps_and_rounds_outwards() {
        let r = crate::Rect::new(-5.0, 1.5, 20.0, 3.2);
        assert_eq!(
            Region::clamp(&r, 10, 10),
            Some(Region {
                x: 0,
                y: 1,
                w: 10,
                h: 4
            })
        );
        assert_eq!(
            Region::clamp(&crate::Rect::new(20.0, 0.0, 5.0, 5.0), 10, 10),
            None
        );
        assert_eq!(
            Region::clamp(&crate::Rect::new(1.0, 1.0, 0.0, 5.0), 10, 10),
            None
        );
    }

    #[test]
    fn blur_smooths_a_checkerboard_and_stays_inside_the_region() {
        let (w, h) = (16, 16);
        let mut d = checker(w, h);
        let original = d.clone();
        let region = Region {
            x: 4,
            y: 4,
            w: 8,
            h: 8,
        };
        blur(&mut d, w, region, 2.0);
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                let inside = (4..12).contains(&x) && (4..12).contains(&y);
                if inside {
                    assert!((i32::from(d[o]) - 128).abs() < 40, "({x},{y}) = {}", d[o]);
                } else {
                    assert_eq!(d[o..o + 4], original[o..o + 4]);
                }
                assert_eq!(d[o + 3], 255);
            }
        }
    }

    #[test]
    fn blur_keeps_a_flat_colour() {
        let mut d = vec![90u8; 8 * 8 * 4];
        blur(
            &mut d,
            8,
            Region {
                x: 0,
                y: 0,
                w: 8,
                h: 8,
            },
            3.0,
        );
        assert!(d.iter().all(|&v| v == 90));
    }

    #[test]
    fn pixelate_averages_cells() {
        let (w, h) = (6, 4);
        let mut d = checker(w, h);
        pixelate(&mut d, w, Region { x: 0, y: 0, w, h }, 2);
        // every 2x2 cell of a checkerboard averages to 127/128
        for chunk in d.as_chunks::<4>().0 {
            assert!((126..=129).contains(&chunk[0]), "{}", chunk[0]);
        }
        // identical pixels inside a cell
        assert_eq!(d[0..4], d[4..8]);
    }

    #[test]
    fn pixelate_handles_partial_cells() {
        let mut d = checker(5, 3);
        pixelate(
            &mut d,
            5,
            Region {
                x: 1,
                y: 0,
                w: 4,
                h: 3,
            },
            3,
        );
        assert_eq!(d.len(), 5 * 3 * 4); // no out-of-bounds write
    }
}
