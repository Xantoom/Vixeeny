// SPDX-License-Identifier: GPL-3.0-or-later
//! The pixels handed to an engine.

/// Tightly packed BGRA, 8 bits per channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl OcrImage {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && bgra.len() == width as usize * height as usize * 4).then_some(
            Self {
                width,
                height,
                bgra,
            },
        )
    }

    /// Scales down by an integer factor (box filter) until both sides fit in `max`. Engines
    /// refuse images beyond their limit (about 10 000 px on Windows).
    pub fn fit_within(self, max: u32) -> Self {
        let factor = self.width.max(self.height).div_ceil(max.max(1));
        if factor <= 1 {
            return self;
        }
        let (w, h) = ((self.width / factor).max(1), (self.height / factor).max(1));
        let f = factor as usize;
        let mut out = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let mut sum = [0u32; 4];
                let mut n = 0u32;
                for dy in 0..f {
                    for dx in 0..f {
                        let (sx, sy) = (x * f + dx, y * f + dy);
                        if sx < self.width as usize && sy < self.height as usize {
                            let o = (sy * self.width as usize + sx) * 4;
                            for (acc, v) in sum.iter_mut().zip(&self.bgra[o..o + 4]) {
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
            width: w,
            height: h,
            bgra: out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_the_buffer() {
        assert!(OcrImage::new(2, 2, vec![0; 16]).is_some());
        assert!(OcrImage::new(2, 2, vec![0; 15]).is_none());
        assert!(OcrImage::new(0, 2, vec![]).is_none());
    }

    #[test]
    fn small_images_are_untouched() {
        let img = OcrImage::new(4, 2, vec![7; 32]).unwrap_or_else(|| unreachable!());
        assert_eq!(img.clone().fit_within(10), img);
    }

    #[test]
    fn large_images_are_averaged_down() {
        // 8×4, left half 0, right half 200 → 4×2 after factor 2 (max 4)
        let mut px = vec![0u8; 8 * 4 * 4];
        for y in 0..4 {
            for x in 4..8 {
                px[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4].fill(200);
            }
        }
        let img = OcrImage::new(8, 4, px)
            .unwrap_or_else(|| unreachable!())
            .fit_within(4);
        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(img.bgra[0], 0);
        assert_eq!(img.bgra[12], 200); // pixel (3, 0): right half
    }
}
