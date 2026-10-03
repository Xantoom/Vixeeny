// SPDX-License-Identifier: GPL-3.0-or-later
//! Images onto the system clipboard.

use vixeeny_image::{Bgra, ImageFormat};
use vixeeny_platform::PlatformError;

/// Lossless copy: a `PNG` entry and a bitmap (see `vixeeny_platform::clipboard`).
pub fn copy_bgra(image: &Bgra<'_>) -> anyhow::Result<()> {
    let png = vixeeny_image::encode(ImageFormat::Png, image, &vixeeny_image::Settings::default())?;
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
    vixeeny_platform::clipboard::copy_image(image.width, image.height, pixels, &png)
        .map_err(|e: PlatformError| anyhow::anyhow!("{e}"))
}
