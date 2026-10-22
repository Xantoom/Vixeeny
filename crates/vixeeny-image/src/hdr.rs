// SPDX-License-Identifier: GPL-3.0-or-later
//! HDR pictures kept as HDR: the scRGB the capture returns for an HDR monitor, written as
//! BT.2020 / PQ (absolute: a pixel keeps the brightness it had on the screen).

use rayon::prelude::*;
use vixeeny_common::colour;

use crate::ImageError;
use crate::tonemap::{ToneMapParams, tonemap_frame};

/// Borrowed scRGB pixels (linear BT.709, `1.0` = 80 nits), 4 floats per pixel, tightly packed.
/// Alpha is ignored.
#[derive(Debug, Clone, Copy)]
pub struct Hdr<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [f32],
    /// SDR white on the screen it was captured from, in nits: the white of the SDR rendering.
    pub sdr_white_nits: f32,
    /// Peak brightness of that screen, in nits.
    pub peak_nits: f32,
}

impl Hdr<'_> {
    pub(crate) fn validate(&self) -> Result<(), ImageError> {
        if self.width == 0 || self.height == 0 {
            return Err(ImageError::BadBuffer("empty image"));
        }
        if self.rgba.len() < self.width as usize * self.height as usize * 4 {
            return Err(ImageError::BadBuffer("buffer smaller than width × height"));
        }
        Ok(())
    }

    /// The SDR rendering (tone-mapped), 8-bit BGRA: the clipboard, the SDR base of Ultra HDR.
    pub fn to_sdr(self) -> Vec<u8> {
        tonemap_frame(
            &self.rgba[..self.width as usize * self.height as usize * 4],
            &ToneMapParams {
                sdr_white_nits: self.sdr_white_nits,
                peak_nits: self.peak_nits,
            },
        )
    }

    /// Tightly packed RGB, BT.2020 / PQ, full range 16 bits.
    pub(crate) fn to_pq(self) -> Vec<u16> {
        let w = self.width as usize;
        let mut out = vec![0; w * self.height as usize * 3];
        out.par_chunks_mut(w * 3)
            .zip(self.rgba.par_chunks(w * 4))
            .for_each(|(out, row)| {
                for (o, px) in out
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0)
                {
                    *o = colour::scrgb_to_pq([px[0], px[1], px[2]]);
                }
            });
        out
    }

    /// [`to_pq`](Self::to_pq) as bytes, each sample laid out by `bytes` (`u16::to_be_bytes` for
    /// PNG): written in place, without a copy of the samples.
    pub(crate) fn to_pq_bytes(self, bytes: fn(u16) -> [u8; 2]) -> Vec<u8> {
        let w = self.width as usize;
        let mut out = vec![0; w * self.height as usize * 6];
        out.par_chunks_mut(w * 6)
            .zip(self.rgba.par_chunks(w * 4))
            .for_each(|(out, row)| {
                for (o, px) in out
                    .as_chunks_mut::<6>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0)
                {
                    let [r, g, b] = colour::scrgb_to_pq([px[0], px[1], px[2]]).map(bytes);
                    *o = [r[0], r[1], g[0], g[1], b[0], b[1]];
                }
            });
        out
    }
}

/// PQ BT.2020 RGB (16 bits, any bit layout `signal` reads as 0…1) → opaque sRGB BGRA, tone-mapped
/// as a capture with SDR white at the BT.2408 reference (203 nits) on a 1000-nit screen. For the
/// thumbnails of HDR files.
pub(crate) fn pq_to_sdr(width: u32, height: u32, signal: &[[f32; 3]]) -> Vec<u8> {
    let scrgb: Vec<f32> = signal
        .par_iter()
        .flat_map_iter(|px| {
            let [r, g, b] = colour::pq_to_scrgb(*px);
            [r, g, b, 1.0]
        })
        .collect();
    debug_assert_eq!(scrgb.len(), width as usize * height as usize * 4);
    tonemap_frame(
        &scrgb,
        &ToneMapParams {
            sdr_white_nits: 203.0,
            peak_nits: 1000.0,
        },
    )
}
