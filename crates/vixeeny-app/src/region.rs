// SPDX-License-Identifier: GPL-3.0-or-later
//! Print Screen flow (plan 5.3): freeze every screen, let the user pick a zone and annotate it,
//! then copy, save or close. The capture happens before the first window exists, so the frozen
//! image never contains the editor.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, WgcBackend};
use vixeeny_common::config::Config;
use vixeeny_common::ipc::ActionId;
use vixeeny_editor::{Command, Rect, RgbaImage, Session};
use vixeeny_image::{Bgra, ImageFormat};
use vixeeny_platform::PlatformError;

use crate::still::{self, Destination, Snapshot};

/// RGBA → BGRA (the layout of the image and clipboard APIs).
fn to_bgra(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len());
    for px in img.data.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

fn copy_to_clipboard(img: &RgbaImage) -> anyhow::Result<()> {
    let bgra = to_bgra(img);
    copy_bgra(&Bgra::new(
        img.width,
        img.height,
        img.width as usize * 4,
        &bgra,
    ))
}

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

fn save(
    config: &Config,
    snap: &Snapshot,
    img: &RgbaImage,
    chosen: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let bgra = to_bgra(img);
    let bitmap = Bgra::new(img.width, img.height, img.width as usize * 4, &bgra);
    let (default_format, settings) = crate::image_output(config);
    if let Some(path) = chosen {
        // "Save as": the extension picks the format.
        let format = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(ImageFormat::from_name)
            .filter(|f| f.available())
            .unwrap_or(default_format);
        let bytes = vixeeny_image::encode(format, &bitmap, &settings)?;
        still::write_atomic(&path, &bytes)?;
        return Ok(path);
    }
    let dir = vixeeny_common::paths::expand_user_dir(&config.paths.images)
        .context("cannot locate the images folder")?;
    let now = vixeeny_platform::local_time();
    let dest = Destination {
        dir: &dir,
        template: &config.paths.filename_template,
        per_app_subfolder: config.paths.per_app_subfolder.images,
        use_foreground_app: config.paths.use_foreground_app,
        app_names: &config.paths.app_names,
        now: &now,
        after_save: None,
    };
    Ok(still::save_image(
        ActionId::CaptureRegion,
        snap,
        &dest,
        (default_format, &settings),
        &vixeeny_platform::exe_metadata,
        &bitmap,
    )?)
}

fn save_as_dialog(config: &Config, window: &vixeeny_ui::EditorWindow) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Vixeeny");
    if let Some(dir) = vixeeny_common::paths::expand_user_dir(&config.paths.images) {
        let _ = std::fs::create_dir_all(&dir);
        dialog = dialog.set_directory(dir);
    }
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        ImageFormat::Avif,
        ImageFormat::Jxl,
    ] {
        if format.available() {
            dialog = dialog.add_filter(format.extension().to_uppercase(), &[format.extension()]);
        }
    }
    // The overlay is always on top: let the dialog appear above it.
    window.set_on_top(false);
    let chosen = dialog.save_file();
    window.set_on_top(true);
    chosen
}

pub fn run(config: &Config) -> anyhow::Result<()> {
    let started = Instant::now();
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let foreground = vixeeny_platform::foreground_window()?;
    let windows = vixeeny_platform::top_level_windows().unwrap_or_default();
    let bounds = vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect))
        .context("no monitor found")?;

    let options = CaptureOptions {
        show_cursor: false,
        tonemap: (config.image.hdr == "tonemap_sdr")
            .then_some(crate::tonemap_hdr as vixeeny_capture::ToneMapFn),
    };
    let mut capturer = Capturer::new(WgcBackend::new()?, monitors.clone());
    let frame = capturer.grab(&CaptureTarget::AllMonitors, options)?;
    let base = RgbaImage::from_bgra(frame.width, frame.height, frame.stride, &frame.data)
        .context("unexpected capture buffer")?;
    drop(frame);

    let zones: Vec<Rect> = windows
        .iter()
        .map(|w| {
            Rect::new(
                (w.rect.x - bounds.x) as f32,
                (w.rect.y - bounds.y) as f32,
                w.rect.width as f32,
                w.rect.height as f32,
            )
        })
        .collect();
    let scale = vixeeny_platform::monitor_at(&monitors, cursor.0, cursor.1)
        .map_or(1.0, |m| m.scale_factor() as f32);
    let snapshot = Snapshot {
        monitors,
        cursor,
        foreground,
    };
    let config = config.clone();

    let mut session = Session::new(base, zones);
    session.dim = f32::from(config.editor.dim_percent.min(90)) / 100.0;
    let overlay = vixeeny_ui::Overlay::new(session, scale, move |command, session, window| {
        let export = || session.export();
        match command {
            Command::Close => true,
            Command::Copy => match export() {
                Some(img) => {
                    if let Err(e) = copy_to_clipboard(&img) {
                        tracing::error!("clipboard: {e:#}");
                        return false;
                    }
                    true
                }
                None => false,
            },
            Command::Save | Command::SaveAs => {
                let Some(img) = export() else { return false };
                let chosen = if command == Command::SaveAs {
                    match save_as_dialog(&config, window) {
                        Some(path) => Some(path),
                        None => return false, // cancelled: stay in the editor
                    }
                } else {
                    None
                };
                match save(&config, &snapshot, &img, chosen) {
                    Ok(path) => {
                        tracing::info!("saved {}", path.display());
                        if config.image.copy_to_clipboard
                            && let Err(e) = copy_to_clipboard(&img)
                        {
                            tracing::warn!("clipboard: {e:#}");
                        }
                        true
                    }
                    Err(e) => {
                        tracing::error!("save failed: {e:#}");
                        false
                    }
                }
            }
            Command::Ocr => {
                tracing::info!("OCR is not implemented yet (M8)");
                false
            }
        }
    })
    .map_err(|e| anyhow::anyhow!("cannot create the editor window: {e}"))?;
    tracing::info!("editor ready after {:?}", started.elapsed());
    overlay
        .run((bounds.x, bounds.y), (bounds.width, bounds.height))
        .map_err(|e| anyhow::anyhow!("editor window: {e}"))
}
