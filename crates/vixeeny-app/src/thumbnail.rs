// SPDX-License-Identifier: GPL-3.0-or-later
//! The thumbnail of a capture, for its notification.

use std::path::Path;

use vixeeny_editor::RgbaImage;

const VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "mkv", "webm"];

/// A thumbnail of a capture, at most `max_side` pixels on its long side. `None` for files that
/// cannot be decoded.
pub fn thumbnail(path: &Path, max_side: u32) -> Option<RgbaImage> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        let (width, height, data) = vixeeny_encode::thumbnail::video_thumbnail(path, max_side)?;
        return Some(RgbaImage {
            width,
            height,
            data,
        });
    }
    let bytes = std::fs::read(path).ok()?;
    let img = vixeeny_image::decode(&bytes).ok()?;
    Some(scaled(
        &vixeeny_image::Bgra::new(img.width, img.height, img.width as usize * 4, &img.bgra),
        max_side,
    ))
}

/// `img` at most `max_side` pixels on its long side.
pub fn scaled(img: &vixeeny_image::Bgra<'_>, max_side: u32) -> RgbaImage {
    let scale = (max_side as f32 / img.width.max(img.height).max(1) as f32).min(1.0);
    let (w, h) = (
        ((img.width as f32 * scale).round() as u32).max(1),
        ((img.height as f32 * scale).round() as u32).max(1),
    );
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    // Box filter: the mean of the source pixels each thumbnail pixel covers.
    for y in 0..h {
        let y0 = (u64::from(y) * u64::from(img.height) / u64::from(h)) as usize;
        let y1 = ((u64::from(y + 1) * u64::from(img.height) / u64::from(h)) as usize).max(y0 + 1);
        for x in 0..w {
            let x0 = (u64::from(x) * u64::from(img.width) / u64::from(w)) as usize;
            let x1 =
                ((u64::from(x + 1) * u64::from(img.width) / u64::from(w)) as usize).max(x0 + 1);
            let (mut b, mut g, mut r, mut n) = (0_u32, 0_u32, 0_u32, 0_u32);
            for sy in y0..y1.min(img.height as usize) {
                for sx in x0..x1.min(img.width as usize) {
                    let o = sy * img.stride + sx * 4;
                    let Some(px) = img.data.get(o..o + 3) else {
                        continue;
                    };
                    b += u32::from(px[0]);
                    g += u32::from(px[1]);
                    r += u32::from(px[2]);
                    n += 1;
                }
            }
            let n = n.max(1);
            out.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, 255]);
        }
    }
    RgbaImage {
        width: w,
        height: h,
        data: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vixeeny-thumb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn thumbnails_are_scaled_down_with_the_aspect_ratio_and_videos_have_none() {
        let root = scratch("thumb");
        let path = root.join("wide.png");
        let mut data = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut data, 400, 100);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            // Left half red, right half blue.
            let row: Vec<u8> = (0..400)
                .flat_map(|x| {
                    if x < 200 {
                        [255, 0, 0, 255]
                    } else {
                        [0, 0, 255, 255]
                    }
                })
                .collect();
            w.write_image_data(&row.repeat(100)).unwrap();
        }
        std::fs::write(&path, data).unwrap();
        let RgbaImage {
            width: w,
            height: h,
            data: rgba,
        } = thumbnail(&path, 100).unwrap();
        assert_eq!((w, h), (100, 25));
        assert_eq!(rgba.len(), 100 * 25 * 4);
        assert_eq!(&rgba[..4], [255, 0, 0, 255]);
        assert_eq!(&rgba[(99) * 4..(99) * 4 + 4], [0, 0, 255, 255]);
        // A small picture is not enlarged.
        assert_eq!(
            thumbnail(&path, 1000).map(|t| (t.width, t.height)),
            Some((400, 100))
        );
        assert!(thumbnail(&root.join("a.mp4"), 100).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_image_in_memory_is_scaled_from_its_rows() {
        // 4×2 BGRA, rows 20 bytes apart: left half blue, right half red.
        let mut data = Vec::new();
        for _ in 0..2 {
            data.extend_from_slice(&[255, 0, 0, 255].repeat(2));
            data.extend_from_slice(&[0, 0, 255, 255].repeat(2));
            data.extend_from_slice(&[9; 4]);
        }
        let thumb = scaled(&vixeeny_image::Bgra::new(4, 2, 20, &data), 2);
        assert_eq!((thumb.width, thumb.height), (2, 1));
        assert_eq!(thumb.data, [0, 0, 255, 255, 255, 0, 0, 255]);
    }
}
