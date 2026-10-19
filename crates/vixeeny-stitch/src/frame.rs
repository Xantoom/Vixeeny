// SPDX-License-Identifier: GPL-3.0-or-later
//! Pixels: tightly packed 4 bytes per pixel (the order does not matter here).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && data.len() == width as usize * height as usize * 4).then_some(
            Self {
                width,
                height,
                data,
            },
        )
    }

    pub(crate) fn row_bytes(&self) -> usize {
        self.width as usize * 4
    }

    pub fn row(&self, y: usize) -> &[u8] {
        let n = self.row_bytes();
        &self.data[y * n..(y + 1) * n]
    }
}

/// A preview of the image made of `rows` (each `width * 4` bytes) fitting `max_width` ×
/// `max_height` (integer factor): one row in `factor` read, its pixels averaged by `factor`.
/// Only what is shown is read: cheap even for a very long image.
pub(crate) fn sampled(width: u32, rows: &[&[u8]], max_width: u32, max_height: u32) -> Frame {
    let height = rows.len() as u32;
    let factor = width
        .div_ceil(max_width.max(1))
        .max(height.div_ceil(max_height.max(1)))
        .max(1) as usize;
    let (w, h) = (
        (width as usize / factor).max(1),
        (height as usize / factor).max(1),
    );
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = rows[(y * factor + factor / 2).min(rows.len() - 1)];
        for x in 0..w {
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for px in row[x * factor * 4..].as_chunks::<4>().0.iter().take(factor) {
                for (acc, v) in sum.iter_mut().zip(px) {
                    *acc += u32::from(*v);
                }
                n += 1;
            }
            out.extend(sum.map(|s| (s / n.max(1)) as u8));
        }
    }
    Frame {
        width: w as u32,
        height: h as u32,
        data: out,
    }
}
