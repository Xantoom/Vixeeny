// SPDX-License-Identifier: GPL-3.0-or-later
//! PNG written on every core: the image is cut in bands, each filtered and deflated on its own and ended on a byte
//! boundary (a sync flush), so that the bands join into one zlib stream, as pigz does. A 4K game frame takes 80 ms
//! rather than 0.45 to 1.2 s with one core, for the same size (a band does not see the 32 KB before it: a few hundred
//! bytes).

use flate2::{Compress, Compression, FlushCompress, Status};
use png::{BitDepth, ColorType, Encoder, SrgbRenderingIntent};
use rayon::prelude::*;

use crate::hdr::Light;
use crate::{Bgra, Hdr, ImageError, PngCompression, PngSettings};

/// RGB PNG tagged sRGB. Alpha is dropped: screen captures are opaque, and RGB compresses faster.
pub(crate) fn encode(image: &Bgra<'_>, settings: &PngSettings) -> Result<Vec<u8>, ImageError> {
    let rows = image.rows().collect::<Vec<_>>();
    let row = |y: usize, out: &mut [u8]| {
        for (o, px) in out.as_chunks_mut::<3>().0.iter_mut().zip(rows[y].as_chunks::<4>().0) {
            *o = [px[2], px[1], px[0]];
        }
    };
    write(image.width, image.height, 3, &row, settings, None)
}

/// 16-bit RGB PNG, BT.2020 / PQ, described by a `cICP` chunk, its light by `cLLI` and the screen it was seen on by
/// `mDCV` (PNG third edition).
pub(crate) fn encode_hdr(image: &Hdr<'_>, settings: &PngSettings) -> Result<Vec<u8>, ImageError> {
    let samples = image.to_pq_bytes(u16::to_be_bytes);
    let n = image.width as usize * 6;
    let row = |y: usize, out: &mut [u8]| out.copy_from_slice(&samples[y * n..(y + 1) * n]);
    let light = image.light();
    write(image.width, image.height, 6, &row, settings, Some(light))
}

/// BT.2020 primaries, PQ, RGB, full range (ITU-T H.273).
const CICP_BT2100_PQ: [u8; 4] = [9, 16, 0, 1];

/// `cLLI`: MaxCLL and MaxFALL in 0.0001 nits (the brightest pixel rounded up, a bound).
fn clli(light: &Light) -> [u8; 8] {
    let units = |nits: f32| f64::from(nits) * 10_000.0;
    let mut out = [0; 8];
    out[..4].copy_from_slice(&(units(light.max_cll).ceil() as u32).to_be_bytes());
    out[4..].copy_from_slice(&(units(light.max_fall).round() as u32).to_be_bytes());
    out
}

/// `mDCV`: red, green, blue and white as x, y in 0.00002, then the brightest and darkest light in 0.0001 nits.
fn mdcv(light: &Light) -> [u8; 24] {
    let mut out = [0; 24];
    let xy = Light::PRIMARIES.iter().chain([&Light::WHITE]).flatten();
    for (o, v) in out.as_chunks_mut::<2>().0.iter_mut().zip(xy) {
        *o = ((v * 50_000.0).round() as u16).to_be_bytes();
    }
    out[16..20].copy_from_slice(&((f64::from(light.peak) * 10_000.0).round() as u32).to_be_bytes());
    out[20..].copy_from_slice(&((Light::BLACK_NITS * 10_000.0).round() as u32).to_be_bytes());
    out
}

/// The fewest rows a band gets: below, the bands' ends cost more than the cores give.
const BAND_ROWS: usize = 64;

/// Writes the image whose row `y` (`bpp` bytes per pixel) `row` puts in its buffer.
fn write(
    width: u32,
    height: u32,
    bpp: usize,
    row: &(dyn Fn(usize, &mut [u8]) + Sync),
    settings: &PngSettings,
    hdr: Option<Light>,
) -> Result<Vec<u8>, ImageError> {
    // zlib-rs levels; at 4K, 2 is a fifth smaller than the png crate's fastest for a quarter more time, and 6 and 9 are
    // as small as its own.
    let (level, adaptive) = match settings.compression {
        PngCompression::Fast => (2, false),
        PngCompression::Default => (6, true),
        PngCompression::High => (9, true),
    };
    let (w, h) = (width as usize, height as usize);
    let bands = h.div_ceil(BAND_ROWS).clamp(1, rayon::current_num_threads() * 2);
    let per_band = h.div_ceil(bands);
    let parts = (0..h.div_ceil(per_band))
        .into_par_iter()
        .map(|i| {
            let rows = i * per_band..((i + 1) * per_band).min(h);
            let last = rows.end == h;
            band(rows, w * bpp, bpp, row, level, adaptive, last)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut zlib = Vec::with_capacity(6 + parts.iter().map(|p| p.deflated.len()).sum::<usize>());
    zlib.extend_from_slice(&[0x78, 0x9c]);
    let mut adler = 1;
    for p in &parts {
        zlib.extend_from_slice(&p.deflated);
        adler = adler32_combine(adler, p.adler, p.len);
    }
    zlib.extend_from_slice(&adler.to_be_bytes());
    drop(parts);

    let mut out = Vec::with_capacity(zlib.len() + 128);
    let mut encoder = Encoder::new(&mut out, width, height);
    encoder.set_color(ColorType::Rgb);
    if hdr.is_some() {
        encoder.set_depth(BitDepth::Sixteen);
    } else {
        encoder.set_depth(BitDepth::Eight);
        encoder.set_source_srgb(SrgbRenderingIntent::Perceptual);
    }
    let mut writer = encoder.write_header()?;
    if let Some(light) = &hdr {
        writer.write_chunk(png::chunk::cICP, &CICP_BT2100_PQ)?;
        writer.write_chunk(png::chunk::mDCV, &mdcv(light))?;
        writer.write_chunk(png::chunk::cLLI, &clli(light))?;
    }
    writer.write_chunk(png::chunk::IDAT, &zlib)?;
    writer.finish()?;
    drop(zlib);

    match settings.oxipng_level {
        None => Ok(out),
        Some(level) => {
            let options = oxipng::Options::from_preset(level.min(6));
            oxipng::optimize_from_memory(&out, &options).map_err(|e| ImageError::Oxipng(e.to_string()))
        }
    }
}

/// A band, deflated, and the Adler-32 and length of what went in.
struct Part {
    deflated: Vec<u8>,
    adler: u32,
    len: u64,
}

/// Filters and deflates `rows` (`stride` bytes each): byte-aligned at its end, or the end of the stream when `last`.
fn band(
    rows: std::ops::Range<usize>,
    stride: usize,
    bpp: usize,
    row: &(dyn Fn(usize, &mut [u8]) + Sync),
    level: u32,
    adaptive: bool,
    last: bool,
) -> Result<Part, ImageError> {
    let mut filtered = Vec::with_capacity(rows.len() * (stride + 1));
    let mut prev = vec![0; stride];
    let mut cur = vec![0; stride];
    let mut trial = vec![0; stride];
    let mut best = vec![0; stride];
    if rows.start > 0 {
        row(rows.start - 1, &mut prev);
    }
    for y in rows {
        row(y, &mut cur);
        let up = (y > 0).then_some(&prev[..]);
        if adaptive {
            // The filter leaving the smallest sum of absolute values, as libpng and the png crate pick it.
            let mut best_cost = u64::MAX;
            let mut best_kind = 0;
            for kind in 0..5 {
                apply(kind, &cur, up, bpp, &mut trial);
                let cost = trial.iter().map(|&v| u64::from(v.cast_signed().unsigned_abs())).sum();
                if cost < best_cost {
                    best_cost = cost;
                    best_kind = kind;
                    std::mem::swap(&mut best, &mut trial);
                }
            }
            filtered.push(best_kind);
            filtered.extend_from_slice(&best);
        } else {
            apply(1, &cur, up, bpp, &mut trial);
            filtered.push(1);
            filtered.extend_from_slice(&trial);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let adler = {
        let mut a = simd_adler32::Adler32::new();
        a.write(&filtered);
        a.finish()
    };
    let flush = if last { FlushCompress::Finish } else { FlushCompress::Sync };
    let mut compress = Compress::new(Compression::new(level), false);
    let mut deflated = Vec::with_capacity(filtered.len() / 2 + 4096);
    loop {
        let read = compress.total_in() as usize;
        let status = compress
            .compress_vec(&filtered[read..], &mut deflated, flush)
            .map_err(|e| png::EncodingError::from(std::io::Error::other(e)))?;
        let all_in = compress.total_in() as usize == filtered.len();
        // The output may have filled up before the flush was written whole.
        let done = if last { status == Status::StreamEnd } else { all_in && deflated.len() < deflated.capacity() };
        if done {
            break;
        }
        deflated.reserve(deflated.capacity().max(4096));
    }
    Ok(Part { deflated, adler, len: filtered.len() as u64 })
}

/// PNG filter `kind` (0 none, 1 sub, 2 up, 3 average, 4 Paeth) of `cur` over `up` (`None`: the first row, all zeros
/// above).
fn apply(kind: u8, cur: &[u8], up: Option<&[u8]>, bpp: usize, out: &mut [u8]) {
    let up_at = |i: usize| up.map_or(0, |u| u[i]);
    match kind {
        0 => out.copy_from_slice(cur),
        1 => {
            out[..bpp].copy_from_slice(&cur[..bpp]);
            for i in bpp..cur.len() {
                out[i] = cur[i].wrapping_sub(cur[i - bpp]);
            }
        }
        2 => match up {
            Some(u) => {
                for ((o, &c), &b) in out.iter_mut().zip(cur).zip(u) {
                    *o = c.wrapping_sub(b);
                }
            }
            None => out.copy_from_slice(cur),
        },
        3 => {
            for i in 0..cur.len() {
                let a = if i >= bpp { u16::from(cur[i - bpp]) } else { 0 };
                out[i] = cur[i].wrapping_sub(u16::midpoint(a, u16::from(up_at(i))) as u8);
            }
        }
        _ => {
            for i in 0..cur.len() {
                let (a, c) = if i >= bpp { (cur[i - bpp], up_at(i - bpp)) } else { (0, 0) };
                out[i] = cur[i].wrapping_sub(paeth(a, up_at(i), c));
            }
        }
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i16::from(a), i16::from(b), i16::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// The Adler-32 of two pieces put end to end, from theirs and the second's length (zlib's `adler32_combine`).
fn adler32_combine(first: u32, second: u32, second_len: u64) -> u32 {
    const BASE: u64 = 65_521;
    let rem = second_len % BASE;
    let a1 = u64::from(first & 0xffff);
    let a2 = u64::from(second & 0xffff);
    let sum1 = (a1 + a2 + BASE - 1) % BASE;
    let sum2 = (rem * a1 % BASE + u64::from(first >> 16) + u64::from(second >> 16) + BASE - rem) % BASE;
    (sum1 | (sum2 << 16)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler_of_pieces_is_the_adler_of_the_whole() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let adler = |d: &[u8]| {
            let mut a = simd_adler32::Adler32::new();
            a.write(d);
            a.finish()
        };
        let (x, y) = data.split_at(123_457);
        assert_eq!(adler32_combine(adler(x), adler(y), y.len() as u64), adler(&data));
        assert_eq!(adler32_combine(1, adler(&data), data.len() as u64), adler(&data));
    }
}
