// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ImageError;

/// Borrowed 8-bit BGRA pixels, rows `stride` bytes apart. Alpha is ignored.
#[derive(Debug, Clone, Copy)]
pub struct Bgra<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: &'a [u8],
}

impl<'a> Bgra<'a> {
    pub fn new(width: u32, height: u32, stride: usize, data: &'a [u8]) -> Self {
        Self {
            width,
            height,
            stride,
            data,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), ImageError> {
        let row = self.width as usize * 4;
        if self.width == 0 || self.height == 0 {
            return Err(ImageError::BadBuffer("empty image"));
        }
        if self.stride < row || self.data.len() < self.stride * (self.height as usize - 1) + row {
            return Err(ImageError::BadBuffer("buffer smaller than width × height"));
        }
        Ok(())
    }

    fn rows(&self) -> impl Iterator<Item = &'a [u8]> + use<'a> {
        let (stride, row, data) = (self.stride, self.width as usize * 4, self.data);
        (0..self.height as usize).map(move |y| &data[y * stride..y * stride + row])
    }

    /// Tightly packed RGB.
    pub(crate) fn to_rgb(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for row in self.rows() {
            for px in row.as_chunks::<4>().0 {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        out
    }
}
