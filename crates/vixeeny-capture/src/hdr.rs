// SPDX-License-Identifier: GPL-3.0-or-later
//! Captures kept as HDR: scRGB frames, cropped and assembled like the 8-bit ones.

use std::sync::OnceLock;

use rayon::prelude::*;
use vixeeny_common::colour;
use vixeeny_platform::{HdrInfo, MonitorInfo, PhysicalRect};

use crate::{CaptureError, CpuFrame};

/// scRGB, linear BT.709 floats where `1.0` is 80 nits, 4 per pixel (R, G, B, A), tightly
/// packed.
#[derive(Debug, Clone, PartialEq)]
pub struct HdrFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
}

/// A capture kept as HDR.
#[derive(Debug, Clone, PartialEq)]
pub struct HdrCapture {
    pub frame: HdrFrame,
    /// SDR white and peak of the screen it shows (of the brightest one, for several).
    pub info: HdrInfo,
}

/// The levels of a capture spanning `monitors`: the highest SDR white and peak among the HDR
/// ones (the others are brought to them). `None` without any HDR monitor.
pub fn reference(monitors: &[MonitorInfo]) -> Option<HdrInfo> {
    monitors
        .iter()
        .filter_map(|m| m.hdr)
        .reduce(|a, b| HdrInfo {
            sdr_white_nits: a.sdr_white_nits.max(b.sdr_white_nits),
            peak_nits: a.peak_nits.max(b.peak_nits),
        })
}

impl HdrFrame {
    /// Opaque black.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: [0.0, 0.0, 0.0, 1.0].repeat(width as usize * height as usize),
        }
    }

    /// An SDR frame as scRGB, its white at `white_nits`: how it shows next to HDR content.
    pub fn from_sdr(frame: &CpuFrame, white_nits: f32) -> Self {
        Self::from_bgra(
            frame.width,
            frame.height,
            frame.stride,
            &frame.data,
            white_nits,
        )
    }

    /// [`from_sdr`](Self::from_sdr) for BGRA rows `stride` bytes apart.
    pub fn from_bgra(width: u32, height: u32, stride: usize, data: &[u8], white_nits: f32) -> Self {
        static LINEAR: OnceLock<[f32; 256]> = OnceLock::new();
        let linear =
            LINEAR.get_or_init(|| std::array::from_fn(|i| colour::srgb_decode(i as f32 / 255.0)));
        let gain = white_nits / 80.0;
        let w = width as usize;
        let mut rgba = vec![0.0; w * height as usize * 4];
        rgba.par_chunks_mut(w * 4)
            .zip(data.par_chunks(stride))
            .for_each(|(out, row)| {
                for (o, px) in out
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0)
                {
                    let c = |i: usize| linear[usize::from(px[i])] * gain;
                    *o = [c(2), c(1), c(0), 1.0];
                }
            });
        Self {
            width,
            height,
            rgba,
        }
    }

    /// Multiplies the light by `gain`.
    pub fn scale(&mut self, gain: f32) {
        if gain != 1.0 {
            self.rgba
                .par_chunks_mut(4 * 4096)
                .for_each(|px| px.iter_mut().for_each(|c| *c *= gain));
        }
    }

    /// `rect`, relative to the frame's top-left corner.
    pub fn crop(&self, rect: &PhysicalRect) -> Result<Self, CaptureError> {
        let bounds = PhysicalRect::new(0, 0, self.width, self.height);
        if rect.is_empty() || bounds.intersection(rect) != Some(*rect) {
            return Err(CaptureError::InvalidRegion);
        }
        let (x, w) = (rect.x as usize * 4, rect.width as usize * 4);
        let rgba = self
            .rgba
            .chunks_exact(self.width as usize * 4)
            .skip(rect.y as usize)
            .take(rect.height as usize)
            .flat_map(|row| &row[x..x + w])
            .copied()
            .collect();
        Ok(Self {
            width: rect.width,
            height: rect.height,
            rgba,
        })
    }

    /// Copies `src` with its top-left corner at `(x, y)`, clipped to this frame.
    pub fn blit(&mut self, src: &Self, x: i32, y: i32) {
        let dest = PhysicalRect::new(x, y, src.width, src.height);
        let Some(clip) = PhysicalRect::new(0, 0, self.width, self.height).intersection(&dest)
        else {
            return;
        };
        let (sx, sy) = ((clip.x - x) as usize, (clip.y - y) as usize);
        let n = clip.width as usize * 4;
        for row in 0..clip.height as usize {
            let from = ((sy + row) * src.width as usize + sx) * 4;
            let to = ((clip.y as usize + row) * self.width as usize + clip.x as usize) * 4;
            self.rgba[to..to + n].copy_from_slice(&src.rgba[from..from + n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdr_white_lands_on_the_requested_level() {
        let mut sdr = CpuFrame::new(2, 1);
        sdr.data[..4].copy_from_slice(&[255, 255, 255, 255]);
        let hdr = HdrFrame::from_sdr(&sdr, 240.0);
        assert_eq!(&hdr.rgba[..4], &[3.0, 3.0, 3.0, 1.0]);
        assert_eq!(&hdr.rgba[4..], &[0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn crop_and_blit_move_whole_pixels() {
        let mut frame = HdrFrame::new(4, 3);
        let mut dot = HdrFrame::new(1, 1);
        dot.rgba = vec![5.0, 6.0, 7.0, 1.0];
        frame.blit(&dot, 2, 1);
        let cut = frame.crop(&PhysicalRect::new(1, 1, 2, 2)).unwrap();
        assert_eq!((cut.width, cut.height), (2, 2));
        assert_eq!(&cut.rgba[4..8], &[5.0, 6.0, 7.0, 1.0]);
        assert!(frame.crop(&PhysicalRect::new(3, 0, 2, 1)).is_err());
    }
}
