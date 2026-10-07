// SPDX-License-Identifier: GPL-3.0-or-later
//! Images onto the system clipboard, in the format chosen for images.

use std::path::PathBuf;

use vixeeny_common::config::Config;
use vixeeny_image::{Bgra, ImageFormat};
use vixeeny_platform::PlatformError;
use vixeeny_platform::clipboard::ImageClip;

/// Copies `image`: as PNG (lossless) or, in another chosen format, as a file of that format
/// (see `vixeeny_platform::clipboard`). A bitmap goes along in every case.
pub fn copy_bgra(config: &Config, image: &Bgra<'_>) -> anyhow::Result<()> {
    let (format, settings) = crate::image_output(config);
    let bytes = vixeeny_image::encode(format, image, &settings)?;
    // The clipboard wants tightly packed rows.
    let row = image.width as usize * 4;
    let packed: Vec<u8>;
    let pixels = if image.stride == row {
        image.data
    } else {
        packed = (0..image.height as usize)
            .flat_map(|y| {
                image.data[y * image.stride..y * image.stride + row]
                    .iter()
                    .copied()
            })
            .collect();
        &packed
    };
    let file = if format == ImageFormat::Png {
        None
    } else {
        Some(clip_file(format, &bytes)?)
    };
    let clip = ImageClip {
        png: (format == ImageFormat::Png).then_some(&bytes[..]),
        jpeg: (format == ImageFormat::Jpeg).then_some(&bytes[..]),
        file: file.as_deref(),
    };
    vixeeny_platform::clipboard::copy_image(image.width, image.height, pixels, clip)
        .map_err(|e: PlatformError| anyhow::anyhow!("{e}"))
}

/// Writes the copied image where the clipboard can point to it. Only the latest copy is kept:
/// the clipboard holds a single image.
fn clip_file(format: ImageFormat, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let dir = vixeeny_common::paths::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("no cache folder"))?
        .join("clipboard");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let now = vixeeny_platform::local_time();
    let path = dir.join(format!(
        "Vixeeny_{}_{}.{}",
        now.date(),
        now.time(),
        format.extension()
    ));
    std::fs::write(&path, bytes)?;
    Ok(path)
}
