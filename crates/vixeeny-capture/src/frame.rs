// SPDX-License-Identifier: GPL-3.0-or-later
//! CPU-side pixel buffers and the pure operations on them (crop, compose).

use vixeeny_platform::PhysicalRect;

use crate::CaptureError;

pub const BYTES_PER_PIXEL: usize = 4;

/// Packed 8-bit BGRA, rows `stride` bytes apart (`stride >= width * 4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
}

impl CpuFrame {
    /// A black, opaque frame.
    pub fn new(width: u32, height: u32) -> Self {
        let stride = width as usize * BYTES_PER_PIXEL;
        let mut data = vec![0u8; stride * height as usize];
        for px in data.as_chunks_mut::<BYTES_PER_PIXEL>().0 {
            px[3] = 255;
        }
        Self {
            width,
            height,
            stride,
            data,
        }
    }

    pub fn from_raw(
        width: u32,
        height: u32,
        stride: usize,
        data: Vec<u8>,
    ) -> Result<Self, CaptureError> {
        if stride < width as usize * BYTES_PER_PIXEL || data.len() < stride * height as usize {
            return Err(CaptureError::Os("inconsistent frame dimensions".into()));
        }
        Ok(Self {
            width,
            height,
            stride,
            data,
        })
    }

    pub fn row(&self, y: u32) -> &[u8] {
        let start = y as usize * self.stride;
        &self.data[start..start + self.width as usize * BYTES_PER_PIXEL]
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = y as usize * self.stride + x as usize * BYTES_PER_PIXEL;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Copies `rect` (frame coordinates). Errors when it is empty or leaves the frame.
    pub fn crop(&self, rect: &PhysicalRect) -> Result<Self, CaptureError> {
        let bounds = PhysicalRect::new(0, 0, self.width, self.height);
        if rect.is_empty() || bounds.intersection(rect) != Some(*rect) {
            return Err(CaptureError::InvalidRegion);
        }
        let (x, y) = (rect.x as usize, rect.y as usize);
        let row_bytes = rect.width as usize * BYTES_PER_PIXEL;
        let mut out = Self::new(rect.width, rect.height);
        for row in 0..rect.height as usize {
            let src = (y + row) * self.stride + x * BYTES_PER_PIXEL;
            let dst = row * out.stride;
            out.data[dst..dst + row_bytes].copy_from_slice(&self.data[src..src + row_bytes]);
        }
        Ok(out)
    }

    /// Draws `src` with its top-left corner at (`x`, `y`); parts outside `self` are clipped.
    pub fn blit(&mut self, src: &Self, x: i32, y: i32) {
        let dest = PhysicalRect::new(x, y, src.width, src.height);
        let Some(clip) = PhysicalRect::new(0, 0, self.width, self.height).intersection(&dest)
        else {
            return;
        };
        let row_bytes = clip.width as usize * BYTES_PER_PIXEL;
        for row in 0..clip.height as usize {
            let sy = (clip.y - y) as usize + row;
            let sx = (clip.x - x) as usize;
            let s = sy * src.stride + sx * BYTES_PER_PIXEL;
            let d = (clip.y as usize + row) * self.stride + clip.x as usize * BYTES_PER_PIXEL;
            self.data[d..d + row_bytes].copy_from_slice(&src.data[s..s + row_bytes]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> CpuFrame {
        let mut f = CpuFrame::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = y as usize * f.stride + x as usize * 4;
                f.data[i..i + 4].copy_from_slice(&[x as u8, y as u8, 7, 255]);
            }
        }
        f
    }

    #[test]
    fn crop_copies_the_right_pixels() {
        let f = gradient(10, 10);
        let c = f.crop(&PhysicalRect::new(3, 4, 2, 2)).unwrap();
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.pixel(0, 0), [3, 4, 7, 255]);
        assert_eq!(c.pixel(1, 1), [4, 5, 7, 255]);
    }

    #[test]
    fn crop_rejects_bad_regions() {
        let f = gradient(10, 10);
        assert!(f.crop(&PhysicalRect::new(8, 8, 5, 5)).is_err());
        assert!(f.crop(&PhysicalRect::new(0, 0, 0, 5)).is_err());
        assert!(f.crop(&PhysicalRect::new(-1, 0, 2, 2)).is_err());
    }

    #[test]
    fn blit_clips() {
        let mut canvas = CpuFrame::new(4, 4);
        let src = gradient(3, 3);
        canvas.blit(&src, -1, 2);
        assert_eq!(canvas.pixel(0, 2), [1, 0, 7, 255]);
        assert_eq!(canvas.pixel(1, 3), [2, 1, 7, 255]);
        assert_eq!(canvas.pixel(3, 3), [0, 0, 0, 255]);
    }

    #[test]
    fn from_raw_validates() {
        assert!(CpuFrame::from_raw(2, 2, 8, vec![0; 16]).is_ok());
        assert!(CpuFrame::from_raw(2, 2, 4, vec![0; 16]).is_err());
        assert!(CpuFrame::from_raw(2, 2, 8, vec![0; 8]).is_err());
    }
}
