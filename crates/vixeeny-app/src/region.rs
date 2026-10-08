// SPDX-License-Identifier: GPL-3.0-or-later
//! Print Screen flow (plan 5.3): freeze every screen, let the user pick a zone and annotate it,
//! then copy, save or close. The capture happens before the first window exists, so the frozen
//! image never contains the editor.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, CpuFrame};
use vixeeny_common::config::Config;
use vixeeny_common::ipc::{ActionId, Frozen};
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

fn copy_to_clipboard(config: &Config, img: &RgbaImage) -> anyhow::Result<()> {
    let bgra = to_bgra(img);
    copy_bgra(
        config,
        &Bgra::new(img.width, img.height, img.width as usize * 4, &bgra),
    )
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
) -> Option<bool> {
    match command {
        Command::Close => Some(true),
        Command::Copy => {
            let img = session.export()?;
            Some(match copy_to_clipboard(config, &img) {
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
                match save_as_dialog(config) {
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
                        && let Err(e) = copy_to_clipboard(config, &img)
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
        Command::Scroll => None,
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
        template: &config.paths.naming.images.template,
        per_app_subfolder: config.paths.per_app_subfolder.images,
        use_foreground_app: config.paths.naming.images.use_foreground_app,
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

fn save_as_dialog(config: &Config) -> Option<PathBuf> {
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
    // The overlay lowers its windows for the time of the dialog.
    dialog.save_file()
}

/// What the zone is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Annotate, then copy or save (Print Screen).
    Editor,
    /// Pick the zone of a scrolling capture.
    Scroll,
}

/// The screens the daemon froze for this capture. They go once the editor covers them, or when
/// this is dropped (whatever happened).
struct Thaw(std::cell::RefCell<Option<Frozen>>);

impl Thaw {
    /// Removes the frozen screens once the compositor has shown what now covers them.
    fn release(&self) {
        if let Some(frozen) = self.0.borrow_mut().take() {
            std::thread::spawn(move || {
                vixeeny_platform::wait_for_composition();
                vixeeny_capture::freeze::release(&frozen);
            });
        }
    }
}

impl Drop for Thaw {
    fn drop(&mut self) {
        if let Some(frozen) = self.0.get_mut().take() {
            vixeeny_capture::freeze::release(&frozen);
        }
    }
}

/// The desktop from the frozen screens, laid out like the monitors, converted straight from
/// the GPU's memory into the editor's image.
fn frozen_desktop(
    frozen: &Frozen,
    monitors: &[vixeeny_platform::MonitorInfo],
    bounds: vixeeny_platform::PhysicalRect,
    config: &Config,
) -> anyhow::Result<RgbaImage> {
    let monitor_of = |s: &vixeeny_common::ipc::FrozenScreen| {
        monitors.iter().find(|m| {
            (m.rect.x, m.rect.y, m.rect.width, m.rect.height) == (s.x, s.y, s.width, s.height)
        })
    };
    anyhow::ensure!(
        frozen.screens.iter().all(|s| monitor_of(s).is_some()),
        "the monitors changed since the screens were frozen"
    );
    let row = bounds.width as usize * 4;
    let covered: u64 = monitors
        .iter()
        .map(|m| u64::from(m.rect.width) * u64::from(m.rect.height))
        .sum();
    let mut data = if covered == u64::from(bounds.width) * u64::from(bounds.height) {
        vec![0; row * bounds.height as usize]
    } else {
        // Between monitors of different sizes: opaque black, as a capture shows it.
        [0, 0, 0, 255].repeat(bounds.width as usize * bounds.height as usize)
    };
    let mut failure = None;
    vixeeny_capture::freeze::read_with(frozen, |s, rows, stride| {
        let at = (s.y - bounds.y) as usize * row + (s.x - bounds.x) as usize * 4;
        let size = (s.width as usize, s.height as usize);
        if !s.hdr {
            vixeeny_editor::bgra_to_rgba(rows, stride, &mut data[at..], row, size);
            return;
        }
        let info = monitor_of(s).and_then(|m| m.hdr);
        let pixels = vixeeny_capture::freeze::FrozenPixels {
            screen: *s,
            stride,
            data: rows.to_vec(),
        };
        let frame = match info {
            Some(info) if config.image.hdr == "tonemap_sdr" => {
                let bgra = crate::tonemap_hdr(&pixels.to_scrgb(), &info);
                CpuFrame::from_raw(s.width, s.height, s.width as usize * 4, bgra)
            }
            info => pixels.to_bgra(info.map_or(80.0, |i| i.sdr_white_nits)),
        };
        match frame {
            Ok(frame) => {
                vixeeny_editor::bgra_to_rgba(&frame.data, frame.stride, &mut data[at..], row, size)
            }
            Err(e) => failure = Some(e),
        }
    })?;
    if let Some(e) = failure {
        return Err(e.into());
    }
    RgbaImage::new(bounds.width, bounds.height, data).context("unexpected image size")
}

pub fn run(config: &Config, mode: Mode, frozen: Option<Frozen>) -> anyhow::Result<()> {
    let started = Instant::now();
    tracing::info!(
        "zone capture ({mode:?}) starting{}",
        if frozen.is_some() {
            " on the frozen screens"
        } else {
            ""
        }
    );
    let thaw = std::rc::Rc::new(Thaw(std::cell::RefCell::new(frozen)));
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let foreground = vixeeny_platform::foreground_window()?;
    tracing::info!(
        "{} monitor(s) after {:?}",
        monitors.len(),
        started.elapsed()
    );
    let bounds = vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect))
        .context("no monitor found")?;

    let from_daemon = thaw.0.borrow().as_ref().and_then(|frozen| {
        frozen_desktop(frozen, &monitors, bounds, config)
            .map_err(|e| tracing::warn!("frozen screens not used: {e:#}"))
            .ok()
    });
    let base = match from_daemon {
        Some(base) => base,
        None => {
            let backend = vixeeny_capture::WgcBackend::new()?;
            let options = CaptureOptions {
                show_cursor: false,
                tonemap: (config.image.hdr == "tonemap_sdr")
                    .then_some(crate::tonemap_hdr as vixeeny_capture::ToneMapFn),
            };
            let mut capturer = Capturer::new(backend, monitors.clone());
            let frame = capturer.grab(&CaptureTarget::AllMonitors, options)?;
            RgbaImage::from_bgra(frame.width, frame.height, frame.stride, &frame.data)
                .context("unexpected capture buffer")?
        }
    };
    tracing::info!(
        "screen read ({}x{}) after {:?}",
        base.width,
        base.height,
        started.elapsed()
    );

    let scale = vixeeny_platform::monitor_at(&monitors, cursor.0, cursor.1)
        .map_or(1.0, |m| m.scale_factor() as f32);
    // One overlay window per monitor, each showing its part of the frozen desktop.
    // The monitors in image pixels: the toolbar and the magnifier stay on one of them.
    let monitor_areas: Vec<Rect> = monitors
        .iter()
        .map(|m| {
            Rect::new(
                (m.rect.x - bounds.x) as f32,
                (m.rect.y - bounds.y) as f32,
                m.rect.width as f32,
                m.rect.height as f32,
            )
        })
        .collect();
    // One window per monitor, exactly over it. Its content is a DirectComposition tree (no swap
    // chain), so Windows never takes it for a full-screen game.
    let screens: Vec<vixeeny_overlay::Screen> = monitors
        .iter()
        .map(|m| vixeeny_overlay::Screen {
            position: (m.rect.x, m.rect.y),
            size: (m.rect.width, m.rect.height),
            area: Rect::new(
                (m.rect.x - bounds.x) as f32,
                (m.rect.y - bounds.y) as f32,
                m.rect.width as f32,
                m.rect.height as f32,
            ),
        })
        .collect();
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
    let tips = toolbar_tips(crate::lang(&config.general.language));
    let config_for_scroll = config.clone();

    let scroll_zone: std::rc::Rc<std::cell::Cell<Option<Rect>>> = std::rc::Rc::default();
    let scroll_slot = scroll_zone.clone();

    // No window detection: the zone is always drawn freely.
    let mut session = Session::new(base, Vec::new()).with_screens(monitor_areas);
    match mode {
        Mode::Editor => {}
        Mode::Scroll => session = session.with_auto_command(Command::Scroll),
    }
    session.dim = f32::from(config.editor.dim_percent.min(90)) / 100.0;
    let mut overlay = vixeeny_overlay::Overlay::on_screens(
        session,
        scale,
        &screens,
        crate::overlay_look(),
        move |command, session| {
            if let Some(close) = output_command(
                &config,
                &snapshot,
                ActionId::CaptureRegion,
                command,
                session,
            ) {
                return close;
            }
            match command {
                Command::Close | Command::Copy | Command::Save | Command::SaveAs => false,
                Command::Scroll => match session.zone() {
                    Some(zone) => {
                        scroll_slot.set(Some(zone));
                        true
                    }
                    None => false,
                },
            }
        },
    )
    .map_err(|e| anyhow::anyhow!("cannot create the editor window: {e}"))?;
    overlay.set_tips(&tips);
    tracing::info!("editor ready after {:?}", started.elapsed());
    // The windows appear with their content, over the frozen screens, which then go.
    let shown = thaw.clone();
    overlay.on_first_frame(move |_| {
        tracing::info!("editor on screen after {:?}", started.elapsed());
        shown.release();
    });
    let result = overlay.run(cursor);
    drop(thaw);
    result.map_err(|e| anyhow::anyhow!("editor window: {e}"))?;
    if let Some(zone) = scroll_zone.take() {
        let zone = vixeeny_platform::PhysicalRect::new(
            bounds.x + zone.x.round() as i32,
            bounds.y + zone.y.round() as i32,
            zone.w.round() as u32,
            zone.h.round() as u32,
        );
        return crate::scroll::run(&config_for_scroll, scroll_snapshot, zone, scale);
    }
    Ok(())
}

/// What the toolbar's buttons do, in their order (see `vixeeny_overlay::layout::BUTTONS`).
pub(crate) fn toolbar_tips(lang: vixeeny_common::i18n::Lang) -> Vec<String> {
    use vixeeny_common::i18n::{Key, tr};
    [
        Key::TipMove,
        Key::TipSelect,
        Key::TipPen,
        Key::TipHighlight,
        Key::TipLine,
        Key::TipArrow,
        Key::TipRectangle,
        Key::TipEllipse,
        Key::TipText,
        Key::TipCounter,
        Key::TipBlur,
        Key::TipPixelate,
        Key::TipStyle,
        Key::TipUndo,
        Key::TipRedo,
        Key::TipCopy,
        Key::TipSave,
        Key::TipSaveAs,
        Key::TipScroll,
        Key::TipClose,
    ]
    .into_iter()
    .map(|k| tr(k, lang).to_owned())
    .collect()
}
