// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask icons`: draws the application icon (a near-black rounded tile with a white V
//! framed by the corners of a capture zone, after the Vixely logo) at every size and writes
//! `packaging/icons/`: PNGs and `vixeeny.ico`.
//! The files are committed; run this again only when the drawing changes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

/// The ink of the tile, the same near-black as the Vixely logo.
const INK: (u8, u8, u8) = (0x13, 0x14, 0x16);
const SIZES: [u32; 9] = [16, 24, 32, 48, 64, 128, 256, 512, 1024];
/// Below this size the capture corners would blur into the letter: the small icons show the V
/// alone, larger.
const DETAILED_FROM: u32 = 32;

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/icons")
}

fn paint(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, a));
    paint.anti_alias = true;
    paint
}

/// The rounded tile on a 32-unit grid.
fn tile() -> Option<tiny_skia::Path> {
    let (min, max, r) = (1.0, 31.0, 7.5);
    let k = 0.552_284_8 * r;
    let mut p = PathBuilder::new();
    p.move_to(min + r, min);
    p.line_to(max - r, min);
    p.cubic_to(max - r + k, min, max, min + r - k, max, min + r);
    p.line_to(max, max - r);
    p.cubic_to(max, max - r + k, max - r + k, max, max - r, max);
    p.line_to(min + r, max);
    p.cubic_to(min + r - k, max, min, max - r + k, min, max - r);
    p.line_to(min, min + r);
    p.cubic_to(min, min + r - k, min + r - k, min, min + r, min);
    p.close();
    p.finish()
}

/// The V, a heavy geometric letter with flat terminals, centred on (16, 16) and `scale` times
/// its size in the detailed icon.
fn letter(scale: f32) -> Option<tiny_skia::Path> {
    const POINTS: [(f32, f32); 7] = [
        (10.2, 10.6),
        (13.5, 10.6),
        (16.0, 17.05),
        (18.5, 10.6),
        (21.8, 10.6),
        (17.6, 21.4),
        (14.4, 21.4),
    ];
    let at = |(x, y): (f32, f32)| (16.0 + (x - 16.0) * scale, 16.0 + (y - 16.0) * scale);
    let mut p = PathBuilder::new();
    let (x, y) = at(POINTS[0]);
    p.move_to(x, y);
    for point in &POINTS[1..] {
        let (x, y) = at(*point);
        p.line_to(x, y);
    }
    p.close();
    p.finish()
}

/// The four corners of a capture zone around the letter.
fn corners() -> Option<tiny_skia::Path> {
    let (lo, hi, arm) = (6.0, 26.0, 4.0);
    let mut p = PathBuilder::new();
    for (x, y, dx, dy) in [
        (lo, lo, 1.0, 1.0),
        (hi, lo, -1.0, 1.0),
        (lo, hi, 1.0, -1.0),
        (hi, hi, -1.0, -1.0),
    ] {
        p.move_to(x, y + dy * arm);
        p.line_to(x, y);
        p.line_to(x + dx * arm, y);
    }
    p.finish()
}

/// The red dot of the "recording" tray icon, ringed with the tile's ink so it stays round on
/// the letter.
fn recording_dot(pixmap: &mut Pixmap, size: u32) -> Result<()> {
    let scale = Transform::from_scale(size as f32 / 32.0, size as f32 / 32.0);
    let circle = |r: f32| PathBuilder::from_circle(24.5, 7.5, r).context("dot");
    pixmap.fill_path(
        &circle(7.5)?,
        &paint(INK.0, INK.1, INK.2, 255),
        FillRule::Winding,
        scale,
        None,
    );
    pixmap.fill_path(
        &circle(5.5)?,
        &paint(0xe5, 0x32, 0x2d, 255),
        FillRule::Winding,
        scale,
        None,
    );
    Ok(())
}

/// The icon at `size` pixels, as RGBA.
pub fn pixmap(size: u32) -> Result<Pixmap> {
    let mut pixmap = Pixmap::new(size, size).context("pixmap")?;
    let scale = Transform::from_scale(size as f32 / 32.0, size as f32 / 32.0);
    pixmap.fill_path(
        &tile().context("tile")?,
        &paint(INK.0, INK.1, INK.2, 255),
        FillRule::Winding,
        scale,
        None,
    );
    let white = paint(255, 255, 255, 255);
    if size >= DETAILED_FROM {
        let stroke = Stroke {
            width: 1.7,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&corners().context("corners")?, &white, &stroke, scale, None);
        pixmap.fill_path(
            &letter(1.12).context("letter")?,
            &white,
            FillRule::Winding,
            scale,
            None,
        );
    } else {
        pixmap.fill_path(
            &letter(1.55).context("letter")?,
            &white,
            FillRule::Winding,
            scale,
            None,
        );
    }
    Ok(pixmap)
}

fn draw(size: u32) -> Result<Vec<u8>> {
    pixmap(size)?.encode_png().context("png")
}

/// A Windows icon holding PNG images (supported since Vista); 256 is written as 0.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}

pub fn run() -> Result<()> {
    let dir = out_dir();
    std::fs::create_dir_all(&dir)?;
    let images: Vec<(u32, Vec<u8>)> = SIZES
        .iter()
        .map(|&size| draw(size).map(|png| (size, png)))
        .collect::<Result<_>>()?;
    for (size, png) in &images {
        std::fs::write(dir.join(format!("vixeeny-{size}.png")), png)?;
    }
    let small: Vec<_> = images.iter().filter(|(s, _)| *s <= 256).cloned().collect();
    std::fs::write(dir.join("vixeeny.ico"), ico(&small))?;
    // The tray icon while recording: the small sizes only, with the red dot.
    let recording: Vec<(u32, Vec<u8>)> = [16, 20, 24, 32, 40, 48, 64]
        .iter()
        .map(|&size| {
            let mut p = pixmap(size)?;
            recording_dot(&mut p, size)?;
            Ok((size, p.encode_png().context("png")?))
        })
        .collect::<Result<_>>()?;
    std::fs::write(dir.join("vixeeny-recording.ico"), ico(&recording))?;
    println!("wrote {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_containers_have_valid_headers() {
        let images = vec![(16, vec![1, 2, 3]), (256, vec![4, 5])];
        let ico = ico(&images);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        // The second entry is 256 px (written as 0) and starts after the first image.
        assert_eq!(ico[6 + 16], 0);
        assert_eq!(
            u32::from_le_bytes(ico[6 + 16 + 12..6 + 16 + 16].try_into().unwrap_or([0; 4])),
            6 + 32 + 3
        );
    }

    #[test]
    fn the_drawing_is_a_png_of_the_right_size() {
        let png = draw(64).unwrap_or_default();
        assert_eq!(&png[1..4], b"PNG");
        // IHDR width and height.
        assert_eq!(
            u32::from_be_bytes(png[16..20].try_into().unwrap_or([0; 4])),
            64
        );
    }
}
