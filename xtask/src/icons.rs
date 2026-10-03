// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask icons`: draws the application icon (the tray icon's violet rounded square with a
//! white play triangle, see `crates/vixeeny-daemon/src/icon.rs`) at every size the packages need
//! and writes `packaging/icons/`: PNGs, `vixeeny.ico` (Windows) and `vixeeny.icns` (macOS).
//! The files are committed; run this again only when the drawing changes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};

const VIOLET: (u8, u8, u8) = (0x7c, 0x5c, 0xff);
const SIZES: [u32; 9] = [16, 24, 32, 48, 64, 128, 256, 512, 1024];

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/icons")
}

fn draw(size: u32) -> Result<Vec<u8>> {
    let mut pixmap = Pixmap::new(size, size).context("pixmap")?;
    let s = size as f32 / 32.0;
    let mut violet = Paint::default();
    violet.set_color(Color::from_rgba8(VIOLET.0, VIOLET.1, VIOLET.2, 255));
    violet.anti_alias = true;
    // Rounded square: 1 px margin and a 7 px radius on a 32 px grid.
    let (min, max, r) = (1.0 * s, 31.0 * s, 7.0 * s);
    let k = 0.552_284_8 * r;
    let mut square = PathBuilder::new();
    square.move_to(min + r, min);
    square.line_to(max - r, min);
    square.cubic_to(max - r + k, min, max, min + r - k, max, min + r);
    square.line_to(max, max - r);
    square.cubic_to(max, max - r + k, max - r + k, max, max - r, max);
    square.line_to(min + r, max);
    square.cubic_to(min + r - k, max, min, max - r + k, min, max - r);
    square.line_to(min, min + r);
    square.cubic_to(min, min + r - k, min + r - k, min, min + r, min);
    square.close();
    pixmap.fill_path(
        &square.finish().context("square")?,
        &violet,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let mut white = Paint::default();
    white.set_color(Color::WHITE);
    white.anti_alias = true;
    let mut triangle = PathBuilder::new();
    triangle.move_to(12.0 * s, 9.5 * s);
    triangle.line_to(12.0 * s, 22.5 * s);
    triangle.line_to(23.0 * s, 16.0 * s);
    triangle.close();
    pixmap.fill_path(
        &triangle.finish().context("triangle")?,
        &white,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    pixmap.encode_png().context("png")
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

/// An Apple icon: `icns`, then (type, length, PNG) entries. The types name the pixel size.
fn icns(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    const TYPES: [(u32, &[u8; 4]); 7] = [
        (16, b"icp4"),
        (32, b"icp5"),
        (64, b"icp6"),
        (128, b"ic07"),
        (256, b"ic08"),
        (512, b"ic09"),
        (1024, b"ic10"),
    ];
    let mut body = Vec::new();
    for (size, kind) in TYPES {
        if let Some((_, png)) = images.iter().find(|(s, _)| *s == size) {
            body.extend_from_slice(kind);
            body.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
            body.extend_from_slice(png);
        }
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
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
    std::fs::write(dir.join("vixeeny.icns"), icns(&images))?;
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
        let icns = icns(&[(16, vec![9; 10]), (1024, vec![8; 4])]);
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(icns[4..8].try_into().unwrap_or([0; 4])) as usize,
            icns.len()
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
