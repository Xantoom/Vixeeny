// SPDX-License-Identifier: GPL-3.0-or-later
//! Images onto the system clipboard, in the format chosen for images.

use std::path::PathBuf;

use vixeeny_common::config::Image;
use vixeeny_image::{Bgra, ImageFormat};
use vixeeny_platform::PlatformError;
use vixeeny_platform::clipboard::ImageClip;

/// Copies `image`: as PNG (lossless) or, in another chosen format, as a file of that format (see
/// `vixeeny_platform::clipboard`). A bitmap goes along in every case. `encoded`: the image already encoded with the
/// image settings (the file just saved), not encoded again then. `name`: the file's name where it is pasted, without
/// the extension (that of the saved file).
pub fn copy_bgra(settings: &Image, image: &Bgra<'_>, encoded: Option<&[u8]>, name: &str) -> anyhow::Result<()> {
    let (format, settings) = crate::image_output(settings);
    let made;
    let bytes = match encoded {
        Some(bytes) => bytes,
        None => {
            made = vixeeny_image::encode(format, image, &settings)?;
            &made[..]
        }
    };
    let file = if format == ImageFormat::Png { None } else { Some(clip_file(format, bytes, name)?) };
    let clip = ImageClip {
        png: (format == ImageFormat::Png).then_some(bytes),
        jpeg: (format == ImageFormat::Jpeg).then_some(bytes),
        file: file.as_deref(),
    };
    vixeeny_platform::clipboard::copy_image(image.width, image.height, image.stride, image.data, clip)
        .map_err(|e: PlatformError| anyhow::anyhow!("{e}"))
}

/// Writes the copied image where the clipboard can point to it. Only the latest copy is kept: the clipboard holds a
/// single image.
fn clip_file(format: ImageFormat, bytes: &[u8], name: &str) -> anyhow::Result<PathBuf> {
    let dir = vixeeny_common::paths::cache_dir().ok_or_else(|| anyhow::anyhow!("no cache folder"))?.join("clipboard");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.{}", vixeeny_common::naming::sanitize(name), format.extension()));
    std::fs::write(&path, bytes)?;
    Ok(path)
}
