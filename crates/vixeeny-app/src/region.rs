// SPDX-License-Identifier: GPL-3.0-or-later
//! Print Screen flow (plan 5.3): freeze every screen, let the user pick a zone and annotate it,
//! then copy, save or close. The capture happens before the first window exists, so the frozen
//! image never contains the editor.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer};
use vixeeny_common::config::Config;
use vixeeny_common::ipc::ActionId;
use vixeeny_editor::{Command, Rect, RgbaImage, Session};
use vixeeny_image::{Bgra, ImageFormat};

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

pub use crate::clipboard::copy_bgra;

fn save(
    config: &Config,
    snap: &Snapshot,
    action: ActionId,
    img: &RgbaImage,
    chosen: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let bgra = to_bgra(img);
    let bitmap = Bgra::new(img.width, img.height, img.width as usize * 4, &bgra);
    save_bgra(config, snap, action, &bitmap, chosen)
}

/// Close, copy, save and save-as, shared by every editor. `Some(close)` when `command` is one
/// of them (`close`: the overlay should go away), `None` otherwise.
pub fn output_command(
    config: &Config,
    snapshot: &Snapshot,
    action: ActionId,
    command: Command,
    session: &Session,
    window: &vixeeny_ui::EditorWindow,
) -> Option<bool> {
    match command {
        Command::Close => Some(true),
        Command::Copy => {
            let img = session.export()?;
            Some(match copy_to_clipboard(&img) {
                Ok(()) => true,
                Err(e) => {
                    tracing::error!("clipboard: {e:#}");
                    false
                }
            })
        }
        Command::Save | Command::SaveAs => {
            let img = session.export()?;
            let chosen = if command == Command::SaveAs {
                match save_as_dialog(config, window) {
                    Some(path) => Some(path),
                    None => return Some(false), // cancelled: stay in the editor
                }
            } else {
                None
            };
            Some(match save(config, snapshot, action, &img, chosen) {
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
            })
        }
        Command::Ocr | Command::Scroll => None,
    }
}

/// Writes `bitmap` to the images folder (or to `chosen`, whose extension picks the format).
pub fn save_bgra(
    config: &Config,
    snap: &Snapshot,
    action: ActionId,
    bitmap: &Bgra<'_>,
    chosen: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let (default_format, settings) = crate::image_output(config);
    if let Some(path) = chosen {
        // "Save as": the extension picks the format.
        let format = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(ImageFormat::from_name)
            .filter(|f| f.available())
            .unwrap_or(default_format);
        let bytes = vixeeny_image::encode(format, bitmap, &settings)?;
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
        action,
        snap,
        &dest,
        (default_format, &settings),
        &vixeeny_platform::exe_metadata,
        bitmap,
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

/// What the zone is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Annotate, then copy or save (Print Screen).
    Editor,
    /// Recognise the text of the zone as soon as it is drawn.
    Ocr,
    /// Pick the zone of a scrolling capture.
    #[cfg(windows)]
    Scroll,
}

pub fn run(config: &Config, mode: Mode) -> anyhow::Result<()> {
    let started = Instant::now();
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let foreground = vixeeny_platform::foreground_window()?;
    let windows = vixeeny_platform::top_level_windows().unwrap_or_default();
    let bounds = vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect))
        .context("no monitor found")?;

    #[cfg(windows)]
    let backend = vixeeny_capture::WgcBackend::new()?;
    #[cfg(target_os = "macos")]
    let backend = vixeeny_capture::SckBackend::new()?;
    #[cfg(target_os = "linux")]
    let backend = vixeeny_capture::LinuxBackend::new(&monitors)?;
    let options = CaptureOptions {
        show_cursor: false,
        #[cfg(windows)]
        tonemap: (config.image.hdr == "tonemap_sdr")
            .then_some(crate::tonemap_hdr as vixeeny_capture::ToneMapFn),
        #[cfg(not(windows))]
        tonemap: None,
    };
    let mut capturer = Capturer::new(backend, monitors.clone());
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
    #[cfg(windows)]
    let scroll_snapshot = Snapshot {
        monitors: monitors.clone(),
        cursor,
        foreground: foreground.clone(),
    };
    let snapshot = Snapshot {
        monitors,
        cursor,
        foreground,
    };
    let config = config.clone();
    let ocr_image: std::rc::Rc<std::cell::RefCell<Option<RgbaImage>>> = std::rc::Rc::default();
    let ocr_slot = ocr_image.clone();
    let ocr_config = config.clone();
    #[cfg(windows)]
    let config_for_scroll = config.clone();

    #[cfg(windows)]
    let scroll_zone: std::rc::Rc<std::cell::Cell<Option<Rect>>> = std::rc::Rc::default();
    #[cfg(windows)]
    let scroll_slot = scroll_zone.clone();

    #[allow(unused_mut)]
    let mut session = Session::new(base, zones);
    match mode {
        Mode::Editor => {}
        Mode::Ocr => session = session.with_auto_command(Command::Ocr),
        #[cfg(windows)]
        Mode::Scroll => session = session.with_auto_command(Command::Scroll),
    }
    session.dim = f32::from(config.editor.dim_percent.min(90)) / 100.0;
    let overlay = vixeeny_ui::Overlay::new(session, scale, move |command, session, window| {
        if let Some(close) = output_command(
            &config,
            &snapshot,
            ActionId::CaptureRegion,
            command,
            session,
            window,
        ) {
            return close;
        }
        match command {
            Command::Close | Command::Copy | Command::Save | Command::SaveAs => false,
            #[cfg(not(windows))]
            Command::Scroll => false,
            #[cfg(windows)]
            Command::Scroll => match session.zone() {
                Some(zone) => {
                    scroll_slot.set(Some(zone));
                    true
                }
                None => false,
            },
            Command::Ocr => match session.export() {
                // The overlay closes first; the result window opens afterwards.
                Some(img) => {
                    *ocr_slot.borrow_mut() = Some(img);
                    true
                }
                None => false,
            },
        }
    })
    .map_err(|e| anyhow::anyhow!("cannot create the editor window: {e}"))?;
    tracing::info!("editor ready after {:?}", started.elapsed());
    overlay
        .run((bounds.x, bounds.y), (bounds.width, bounds.height))
        .map_err(|e| anyhow::anyhow!("editor window: {e}"))?;
    #[cfg(windows)]
    if let Some(zone) = scroll_zone.take() {
        let zone = vixeeny_platform::PhysicalRect::new(
            bounds.x + zone.x.round() as i32,
            bounds.y + zone.y.round() as i32,
            zone.w.round() as u32,
            zone.h.round() as u32,
        );
        return crate::scroll::run(&config_for_scroll, scroll_snapshot, zone, scale);
    }
    let recognised = ocr_image.borrow_mut().take();
    if let Some(img) = recognised {
        crate::ocr::run(&img, &ocr_config)?;
    }
    Ok(())
}
