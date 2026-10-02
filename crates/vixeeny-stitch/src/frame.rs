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

    /// Averages down to fit `max_width` × `max_height` (integer factor), for the live preview.
    pub fn thumbnail(&self, max_width: u32, max_height: u32) -> Self {
        let factor = self
            .width
            .div_ceil(max_width.max(1))
            .max(self.height.div_ceil(max_height.max(1)))
            .max(1) as usize;
        if factor == 1 {
            return self.clone();
        }
        let (w, h) = (
            (self.width as usize / factor).max(1),
            (self.height as usize / factor).max(1),
        );
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 4];
                let mut n = 0u32;
                for dy in 0..factor {
                    for dx in 0..factor {
                        let (sx, sy) = (x * factor + dx, y * factor + dy);
                        if sx < self.width as usize && sy < self.height as usize {
                            let o = (sy * self.width as usize + sx) * 4;
                            for (acc, v) in sum.iter_mut().zip(&self.data[o..o + 4]) {
                                *acc += u32::from(*v);
                            }
                            n += 1;
                        }
                    }
                }
                out.extend(sum.map(|s| (s / n.max(1)) as u8));
            }
        }
        Self {
            width: w as u32,
            height: h as u32,
            data: out,
        }
    }
}
