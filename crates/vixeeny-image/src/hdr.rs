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

    /// How bright the picture is, for the readers that map it onto their own screen.
    pub(crate) fn light(self) -> Light {
        let w = self.width as usize;
        let pixels = &self.rgba[..w * self.height as usize * 4];
        let (brightest, sum) = pixels
            .par_chunks(w * 4)
            .map(|row| {
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .fold((0.0_f32, 0.0_f64), |(top, sum), px| {
                        let [r, g, b] = colour::scrgb_to_bt2020([px[0], px[1], px[2]]);
                        // As the file holds it: PQ clips above its peak, and NaN is black.
                        let top_channel = r.max(g).max(b);
                        let nits = if top_channel.is_nan() {
                            0.0
                        } else {
                            (top_channel * 80.0).clamp(0.0, colour::PQ_PEAK_NITS)
                        };
                        (top.max(nits), sum + f64::from(nits))
                    })
            })
            .reduce(|| (0.0, 0.0), |a, b| (a.0.max(b.0), a.1 + b.1));
        Light {
            max_cll: brightest,
            max_fall: (sum / (pixels.len() / 4) as f64) as f32,
            peak: self.peak_nits.clamp(80.0, colour::PQ_PEAK_NITS),
        }
    }

    /// Tightly packed RGB, BT.2020 / PQ, full range 16 bits.
    pub(crate) fn to_pq(self) -> Vec<u16> {
        let w = self.width as usize;
        let mut out = vec![0; w * self.height as usize * 3];
        out.par_chunks_mut(w * 3)
            .zip(self.rgba.par_chunks(w * 4))
            .for_each(|(out, row)| {
                let pixels = out
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0);
                for (o, px) in pixels {
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
                let pixels = out
                    .as_chunks_mut::<6>()
                    .0
                    .iter_mut()
                    .zip(row.as_chunks::<4>().0);
                for (o, px) in pixels {
                    let [r, g, b] = colour::scrgb_to_pq([px[0], px[1], px[2]]).map(bytes);
                    *o = [r[0], r[1], g[0], g[1], b[0], b[1]];
                }
            });
        out
    }
}

/// How bright an HDR picture is (CTA-861.3), in nits, and the screen it was seen on (SMPTE ST
/// 2086): the readers that tone map it to their own screen start from these rather than the
/// 10 000 nits PQ can hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Light {
    /// The brightest channel of the brightest pixel (MaxCLL), as BT.2020.
    pub max_cll: f32,
    /// The mean over the picture of the pixels' brightest channels (MaxFALL).
    pub max_fall: f32,
    /// The screen's peak: the mastering display's maximum luminance.
    pub peak: f32,
}

impl Light {
    /// The mastering display: BT.2020 primaries, as the pixels are (the screen's own are not
    /// read), red, green and blue, then the D65 white, as CIE 1931 x, y.
    pub const PRIMARIES: [[f64; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];
    pub const WHITE: [f64; 2] = [0.3127, 0.3290];
    /// The screen's black is not read either: the usual 0.0001 nits.
    pub const BLACK_NITS: f64 = 0.0001;
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
