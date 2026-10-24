// SPDX-License-Identifier: GPL-3.0-or-later
//! Print Screen flow: freeze every screen, let the user pick a zone and annotate it,
//! then copy, save or close. The capture happens before the first window exists, so the frozen
//! image never contains the editor.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use vixeeny_capture::freeze::Rows;
use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, HdrCapture, HdrFrame};
use vixeeny_common::config::Config;
use vixeeny_common::ipc::{ActionId, Frozen};
use vixeeny_editor::{Command, Rect, RgbaImage, Session};
use vixeeny_image::{Bgra, ImageFormat};
use vixeeny_platform::{HdrInfo, MonitorInfo};

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
        None,
    )
}

pub use crate::clipboard::copy_bgra;

fn save(
    config: &Config,
    snap: &Snapshot,
    action: ActionId,
    img: &RgbaImage,
    hdr: Option<&HdrCapture>,
    chosen: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let bgra = to_bgra(img);
    let bitmap = Bgra::new(img.width, img.height, img.width as usize * 4, &bgra);
    save_bgra(config, snap, action, &bitmap, hdr, chosen)
}

/// The exported zone as HDR: the screens' light where the annotations left the picture
/// untouched, the rest (annotations, and what they blur or cover) at SDR white.
fn annotated_hdr(
    session: &Session,
    exported: &RgbaImage,
    desktop: &HdrCapture,
) -> Option<HdrCapture> {
    use vixeeny_common::colour::srgb_decode;

    let base = session.base();
    let (bw, bh) = (base.width as usize, base.height as usize);
    let r = vixeeny_editor::effects::Region::clamp(&session.zone()?, bw, bh)?;
    let same_size = (desktop.frame.width, desktop.frame.height) == (base.width, base.height)
        && (r.w, r.h) == (exported.width as usize, exported.height as usize);
    if !same_size {
        return None;
    }
    let gain = desktop.info.sdr_white_nits / 80.0;
    let mut rgba = Vec::with_capacity(r.w * r.h * 4);
    for y in 0..r.h {
        for x in 0..r.w {
            let b = ((r.y + y) * bw + r.x + x) * 4;
            let shown = &exported.data[(y * r.w + x) * 4..][..4];
            if shown == &base.data[b..b + 4] {
                rgba.extend_from_slice(&desktop.frame.rgba[b..b + 4]);
            } else {
                let c = |i: usize| srgb_decode(f32::from(shown[i]) / 255.0) * gain;
                rgba.extend_from_slice(&[c(0), c(1), c(2), 1.0]);
            }
        }
    }
    Some(HdrCapture {
        frame: HdrFrame {
            width: r.w as u32,
            height: r.h as u32,
            rgba,
        },
        info: desktop.info,
    })
}

/// Close, copy, save and save-as, shared by every editor. `Some(close)` when `command` is one
/// of them (`close`: the overlay should go away), `None` otherwise. `hdr`: the editor's
/// picture as HDR, when HDR is kept.
pub fn output_command(
    config: &Config,
    snapshot: &Snapshot,
    action: ActionId,
    command: Command,
    session: &Session,
    hdr: Option<&HdrCapture>,
) -> Option<bool> {
    match command {
        Command::Close => Some(true),
        Command::Copy => {
            let img = session.export()?;
            // The picture is in hand: the editor goes while it is encoded.
            vixeeny_overlay::step_aside();
            Some(match copy_to_clipboard(config, &img) {
                Ok(()) => true,
                Err(e) => {
                    tracing::error!("clipboard: {e:#}");
                    false
                }
            })
        }
        Command::CopyText => {
            let img = session.export()?;
            copy_text(config, &img);
            Some(true)
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
            vixeeny_overlay::step_aside();
            let hdr = hdr.and_then(|desktop| annotated_hdr(session, &img, desktop));
            Some(
                match save(config, snapshot, action, &img, hdr.as_ref(), chosen) {
                    Ok(path) => {
                        tracing::debug!("saved {}", path.display());
                        true
                    }
                    Err(e) => {
                        tracing::error!("save failed: {e:#}");
                        false
                    }
                },
            )
        }
        Command::Scroll => None,
    }
}

/// Reads the text of `img` and puts it on the clipboard; a card says what was copied, or why
/// nothing was.
fn copy_text(config: &Config, img: &RgbaImage) {
    use crate::toast::{Failed, Toast, notify};
    use vixeeny_common::i18n::{Key, tr};
    use vixeeny_platform::ocr::{OcrError, recognize};

    let lang = crate::lang(&config.general.language);
    let toast = match recognize(img.width, img.height, &to_bgra(img)) {
        Ok(text) if text.trim().is_empty() => Toast::Failed(Failed::NoText, String::new()),
        Ok(text) => match vixeeny_platform::clipboard::copy_text(&text) {
            Ok(()) => Toast::TextCopied(text),
            Err(e) => Toast::Failed(Failed::Ocr, e.to_string()),
        },
        Err(OcrError::NoLanguage) => {
            Toast::Failed(Failed::Ocr, tr(Key::ToastOcrNoLanguage, lang).to_owned())
        }
        Err(e) => {
            tracing::error!("{e}");
            Toast::Failed(Failed::Ocr, e.to_string())
        }
    };
    notify(config, &toast);
}

/// Writes `bitmap` to the images folder (or to `chosen`, whose extension picks the format);
/// `hdr`, the same picture as HDR, is written instead when HDR is kept and the format can. It
/// is then copied to the clipboard when the settings say so.
pub fn save_bgra(
    config: &Config,
    snap: &Snapshot,
    action: ActionId,
    bitmap: &Bgra<'_>,
    hdr: Option<&HdrCapture>,
    chosen: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let (default_format, settings) = crate::image_output(config);
    // Once saved, the image goes to the clipboard as well when the settings say so.
    let copy = |image: &Bgra<'_>, encoded: Option<&[u8]>| {
        if config.image.copy_to_clipboard
            && let Err(e) = copy_bgra(config, image, encoded)
        {
            tracing::warn!("clipboard: {e:#}");
        }
    };
    let output = |format| still::Output {
        format,
        settings: &settings,
        keep_hdr: crate::keep_hdr(config),
        backdrop: None,
    };
    if let Some(path) = chosen {
        // "Save as": the extension picks the format.
        let format = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(ImageFormat::from_name)
            .filter(|f| f.available())
            .unwrap_or(default_format);
        let hdr = hdr.filter(|_| output(format).hdr());
        // The file is the clipboard's image when it is in the format of the settings; else
        // the clipboard's is encoded side by side with it.
        let same = hdr.is_none() && format == default_format;
        return std::thread::scope(|scope| {
            if !same {
                scope.spawn(|| copy(bitmap, None));
            }
            let bytes = match hdr {
                Some(hdr) => vixeeny_image::encode_hdr(format, &still::hdr_image(hdr), &settings)?,
                None => vixeeny_image::encode(format, bitmap, &settings)?,
            };
            still::write_atomic(&path, &bytes)?;
            if same {
                copy(bitmap, Some(&bytes));
            }
            Ok(path)
        });
    }
    let dir = vixeeny_common::paths::ready_dir(
        &config.paths.images,
        vixeeny_common::paths::DEFAULT_IMAGES,
    )
    .context("cannot make the images folder")?;
    let now = vixeeny_platform::local_time();
    let dest = Destination {
        dir: &dir,
        template: &config.paths.naming.images.template,
        per_app_subfolder: config.paths.per_app_subfolder.images,
        use_foreground_app: true,
        app_names: &config.paths.app_names,
        now: &now,
        after_save: Some(&copy),
    };
    let output = output(default_format);
    let metadata = &vixeeny_platform::exe_metadata;
    Ok(match hdr.filter(|_| output.hdr()) {
        Some(hdr) => still::save_hdr(action, snap, &dest, output, metadata, hdr)?,
        None => still::save_image(action, snap, &dest, output, metadata, bitmap)?,
    })
}

fn save_as_dialog(config: &Config) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Vixeeny");
    if let Some(dir) = vixeeny_common::paths::ready_dir(
        &config.paths.images,
        vixeeny_common::paths::DEFAULT_IMAGES,
    ) {
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
/// the GPU's memory into the editor's image (HDR screens tone-mapped). With `keep_hdr`, also
/// the desktop as HDR when a screen shows HDR.
fn frozen_desktop(
    frozen: &Frozen,
    monitors: &[vixeeny_platform::MonitorInfo],
    bounds: vixeeny_platform::PhysicalRect,
    keep_hdr: bool,
) -> anyhow::Result<(RgbaImage, Option<HdrCapture>)> {
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
    let reference = vixeeny_capture::reference(monitors).filter(|_| keep_hdr);
    let mut canvas = reference.map(|_| HdrFrame::new(bounds.width, bounds.height));
    let levels = |s: &vixeeny_common::ipc::FrozenScreen| {
        monitor_of(s).and_then(|m| m.hdr).unwrap_or(HdrInfo {
            sdr_white_nits: 80.0,
            peak_nits: 1000.0,
        })
    };
    // The HDR screens tone-mapped on the graphics card (the others are done here).
    let mut toned = Vec::new();
    let halves = canvas.is_some();
    vixeeny_capture::freeze::read_with(
        frozen,
        |s| Some(levels(s)),
        halves,
        |s, kind, rows, stride| {
            let at = (s.y - bounds.y) as usize * row + (s.x - bounds.x) as usize * 4;
            let size = (s.width as usize, s.height as usize);
            let (x, y) = ((s.x - bounds.x) as u32, (s.y - bounds.y) as u32);
            match kind {
                Rows::Bgra => {
                    vixeeny_editor::bgra_to_rgba(rows, stride, &mut data[at..], row, size);
                    if s.hdr {
                        toned.push(s.texture);
                    } else if let (Some(canvas), Some(reference)) = (&mut canvas, reference) {
                        let white = reference.sdr_white_nits;
                        canvas.put_bgra(x, y, (s.width, s.height), stride, rows, white);
                    }
                }
                Rows::Halves => {
                    let info = levels(s);
                    if let (Some(canvas), Some(reference)) = (&mut canvas, reference) {
                        let gain = reference.sdr_white_nits / info.sdr_white_nits.max(1.0);
                        canvas.put_halves(x, y, (s.width, s.height), stride, rows, gain);
                    }
                    if !toned.contains(&s.texture) {
                        let scrgb = vixeeny_capture::freeze::scrgb(s, rows, stride);
                        let bgra = crate::tonemap_hdr(&scrgb.rgba, &info);
                        vixeeny_editor::bgra_to_rgba(&bgra, size.0 * 4, &mut data[at..], row, size);
                    }
                }
            }
        },
    )?;
    // The editor draws with this device rather than making one more.
    if let Some(device) = vixeeny_capture::freeze::device(frozen.adapter) {
        vixeeny_overlay::gfx::offer_device(device);
    }
    let image =
        RgbaImage::new(bounds.width, bounds.height, data).context("unexpected image size")?;
    let hdr = canvas
        .zip(reference)
        .map(|(frame, info)| HdrCapture { frame, info });
    Ok((image, hdr))
}

/// The screens as they are now, for the editor shown live.
/// At most this many pictures a second of the screens followed live.
const LIVE_INTERVAL: std::time::Duration = std::time::Duration::from_micros(16_667);

/// A part of a monitor followed live (physical pixels).
struct LivePart {
    /// Where it is in the editor's image.
    at: (u32, u32),
    /// Where it is in the monitor's picture.
    from: (u32, u32),
    size: (u32, u32),
}

struct LiveScreens {
    monitors: Vec<MonitorInfo>,
    /// The top-left corner of the desktop: the editor's image starts there.
    origin: (i32, i32),
    keep_hdr: bool,
    /// The HDR picture exported along (see [`output_command`]).
    hdr: std::rc::Rc<std::cell::RefCell<Option<HdrCapture>>>,
    /// Each monitor followed: where the part followed is in the editor's image, where it is in
    /// the monitor's picture, and its size.
    streams: std::cell::RefCell<Vec<(LivePart, vixeeny_capture::VideoStream)>>,
}

impl vixeeny_overlay::Screens for LiveScreens {
    fn grab(&self) -> Option<RgbaImage> {
        match grab_screens(&self.monitors, self.keep_hdr) {
            Ok((picture, hdr)) => {
                *self.hdr.borrow_mut() = hdr;
                Some(picture)
            }
            Err(e) => {
                tracing::warn!("live screens not read: {e:#}");
                None
            }
        }
    }

    fn watch(
        &self,
        area: Option<(u32, u32, u32, u32)>,
        wake: vixeeny_overlay::Wake,
        device: &vixeeny_overlay::ID3D11Device,
    ) {
        let mut streams = self.streams.borrow_mut();
        streams.clear();
        let Some((x, y, w, h)) = area else {
            return;
        };
        let (ox, oy) = self.origin;
        let area = vixeeny_platform::PhysicalRect {
            x: ox + x as i32,
            y: oy + y as i32,
            width: w,
            height: h,
        };
        for monitor in &self.monitors {
            let Some(part) = monitor.rect.intersection(&area) else {
                continue;
            };
            // The whole monitor is captured; only the part followed is copied, on the
            // graphics card, by the editor.
            let target = vixeeny_capture::StreamTarget::Monitor(monitor.clone());
            let wake = wake.clone();
            match vixeeny_capture::VideoStream::start_held(
                &target,
                device.clone(),
                Some(LIVE_INTERVAL),
                Box::new(move || wake.wake()),
            ) {
                Ok(stream) => streams.push((
                    LivePart {
                        at: ((part.x - ox) as u32, (part.y - oy) as u32),
                        from: (
                            (part.x - monitor.rect.x) as u32,
                            (part.y - monitor.rect.y) as u32,
                        ),
                        size: (part.width, part.height),
                    },
                    stream,
                )),
                Err(e) => tracing::warn!("live screens not followed: {e}"),
            }
        }
    }

    fn take(&self) -> Vec<vixeeny_overlay::ScreenPart> {
        let mut parts = Vec::new();
        for (part, stream) in self.streams.borrow().iter() {
            // Only the newest picture waits.
            if let Some(frame) = stream.take_held() {
                let (width, height) = (
                    part.size.0.min(frame.content.0.saturating_sub(part.from.0)),
                    part.size.1.min(frame.content.1.saturating_sub(part.from.1)),
                );
                parts.push(vixeeny_overlay::ScreenPart {
                    x: part.at.0,
                    y: part.at.1,
                    width,
                    height,
                    texture: frame.texture.clone(),
                    from: part.from,
                    keep: Box::new(frame),
                });
            }
        }
        parts
    }
}

/// A picture of every screen (HDR screens tone-mapped), and with `keep_hdr` the same as HDR.
fn grab_screens(
    monitors: &[MonitorInfo],
    keep_hdr: bool,
) -> anyhow::Result<(RgbaImage, Option<HdrCapture>)> {
    let mut backend = crate::capture_backend::Lent::take()?;
    let mut capturer = Capturer::new(&mut *backend, monitors.to_vec());
    let (frame, hdr) = if keep_hdr {
        capturer.grab_all_both(false, crate::tonemap_hdr)?
    } else {
        let options = CaptureOptions {
            show_cursor: false,
            tonemap: Some(crate::tonemap_hdr),
        };
        (capturer.grab(&CaptureTarget::AllMonitors, options)?, None)
    };
    let base = RgbaImage::from_bgra(frame.width, frame.height, frame.stride, &frame.data)
        .context("unexpected capture buffer")?;
    Ok((base, hdr))
}

pub fn run(config: &Config, mode: Mode, frozen: Option<Frozen>) -> anyhow::Result<()> {
    let started = Instant::now();
    tracing::debug!(
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
    tracing::debug!(
        "{} monitor(s) after {:?}",
        monitors.len(),
        started.elapsed()
    );
    let bounds = vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect))
        .context("no monitor found")?;

    let keep_hdr = crate::keep_hdr(config);
    let from_daemon = thaw.0.borrow().as_ref().and_then(|frozen| {
        frozen_desktop(frozen, &monitors, bounds, keep_hdr)
            .map_err(|e| tracing::warn!("frozen screens not used: {e:#}"))
            .ok()
    });
    let (base, hdr) = match from_daemon {
        Some(desktop) => desktop,
        None => grab_screens(&monitors, keep_hdr)?,
    };
    tracing::debug!(
        "screen read ({}x{}) after {:?}",
        base.width,
        base.height,
        started.elapsed()
    );

    let scale = vixeeny_platform::monitor_at(&monitors, cursor.0, cursor.1)
        .map_or(1.0, |m| m.scale_factor() as f32);
    // One overlay window per monitor, each showing its part of the frozen desktop.
    // The monitors in image pixels: the toolbar stays on one of them.
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
    let live_monitors = monitors.clone();
    let snapshot = Snapshot {
        monitors,
        cursor,
        foreground,
    };
    // Live, the export takes a fresh picture of the screens: its HDR twin replaces this one.
    let hdr = std::rc::Rc::new(std::cell::RefCell::new(hdr));
    let live_hdr = hdr.clone();
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
                hdr.borrow().as_ref(),
            ) {
                return close;
            }
            match command {
                Command::Close
                | Command::Copy
                | Command::CopyText
                | Command::Save
                | Command::SaveAs => false,
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
    if mode == Mode::Editor {
        overlay.set_screens(LiveScreens {
            monitors: live_monitors,
            origin: (bounds.x, bounds.y),
            keep_hdr,
            hdr: live_hdr,
            streams: std::cell::RefCell::default(),
        });
    }
    tracing::debug!("editor ready after {:?}", started.elapsed());
    // The windows appear with their content, over the frozen screens, which then go.
    let shown = thaw.clone();
    overlay.on_first_frame(move |_| {
        tracing::debug!("editor on screen after {:?}", started.elapsed());
        shown.release();
        // Unfrozen, the editor takes a fresh picture of the screens: its capture device is
        // made now that the editor shows (not to slow it down).
        if matches!(mode, Mode::Editor) {
            crate::capture_backend::warm();
        }
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

/// Opens the editor on a picture already saved (`--edit <file>`), in a process of its own.
pub fn open_editor(path: &std::path::Path) {
    vixeeny_platform::allow_foreground_handoff();
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("--edit")
            .arg(path)
            .spawn()
    });
    if let Err(e) = spawned {
        tracing::error!("cannot start the editor: {e}");
    }
}

/// Where a `picture` (width, height) goes on the desktop `bounds`, relative to it: centred on
/// the monitor under the cursor when it fits there, else on the whole desktop; `None` when it
/// is larger than the desktop.
fn place_picture(picture: (u32, u32), bounds: &Rect, monitor: &Rect) -> Option<(u32, u32)> {
    let (w, h) = (picture.0 as f32, picture.1 as f32);
    let on = |r: &Rect| {
        (w <= r.w && h <= r.h).then(|| {
            (
                (r.x + ((r.w - w) / 2.0).floor()) as u32,
                (r.y + ((r.h - h) / 2.0).floor()) as u32,
            )
        })
    };
    on(monitor).or_else(|| on(&Rect::new(0.0, 0.0, bounds.w, bounds.h)))
}

/// `--edit <file>`: the picture in the editor, over a dark backdrop, the zone around it: crop
/// it, annotate it, then copy it or save it as a new capture (the file itself is kept).
pub fn run_edit(args: &[String]) -> anyhow::Result<()> {
    let path = PathBuf::from(args.first().context("usage: --edit <file>")?);
    let config = vixeeny_common::paths::config_file()
        .and_then(|p| Config::load(&p).ok())
        .unwrap_or_default();
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let picture = vixeeny_image::decode(&bytes)?;
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let desktop = vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect))
        .context("no monitor found")?;
    let monitor =
        vixeeny_platform::monitor_at(&monitors, cursor.0, cursor.1).context("no monitor found")?;
    let scale = monitor.scale_factor() as f32;
    let area = |r: &vixeeny_platform::PhysicalRect| {
        Rect::new(
            (r.x - desktop.x) as f32,
            (r.y - desktop.y) as f32,
            r.width as f32,
            r.height as f32,
        )
    };
    let bounds = Rect::new(0.0, 0.0, desktop.width as f32, desktop.height as f32);
    let at = place_picture(
        (picture.width, picture.height),
        &bounds,
        &area(&monitor.rect),
    )
    .context("the picture is larger than the screens")?;
    // The backdrop, the picture pasted on it.
    let mut base = RgbaImage::new(
        desktop.width,
        desktop.height,
        [32, 32, 32, 255].repeat(desktop.width as usize * desktop.height as usize),
    )
    .context("unexpected desktop size")?;
    let row = desktop.width as usize * 4;
    vixeeny_editor::bgra_to_rgba(
        &picture.bgra,
        picture.width as usize * 4,
        &mut base.data[at.1 as usize * row + at.0 as usize * 4..],
        row,
        (picture.width as usize, picture.height as usize),
    );
    let monitor_areas: Vec<Rect> = monitors.iter().map(|m| area(&m.rect)).collect();
    let screens: Vec<vixeeny_overlay::Screen> = monitors
        .iter()
        .map(|m| vixeeny_overlay::Screen {
            position: (m.rect.x, m.rect.y),
            size: (m.rect.width, m.rect.height),
            area: area(&m.rect),
        })
        .collect();
    let session = Session::new(base, Vec::new())
        .with_screens(monitor_areas)
        .within(Rect::new(
            at.0 as f32,
            at.1 as f32,
            picture.width as f32,
            picture.height as f32,
        ));
    let snapshot = Snapshot {
        monitors,
        cursor,
        foreground: None,
    };
    let tips = toolbar_tips(crate::lang(&config.general.language));
    let overlay = vixeeny_overlay::Overlay::on_screens(
        session,
        scale,
        &screens,
        crate::overlay_look(),
        move |command, session| {
            output_command(
                &config,
                &snapshot,
                ActionId::CaptureRegion,
                command,
                session,
                None,
            )
            .unwrap_or(false)
        },
    )
    .map_err(|e| anyhow::anyhow!("cannot create the editor window: {e}"))?;
    overlay.set_tips(&tips);
    overlay
        .run(cursor)
        .map_err(|e| anyhow::anyhow!("editor window: {e}"))
}

/// What the toolbar's buttons do, in the order of `vixeeny_overlay::layout::Tip`.
pub(crate) fn toolbar_tips(lang: vixeeny_common::i18n::Lang) -> Vec<String> {
    use vixeeny_common::i18n::{Key, tr};
    [
        Key::TipMove,
        Key::TipLive,
        Key::TipFreeze,
        Key::TipPen,
        Key::TipHighlight,
        Key::TipArrow,
        Key::TipLine,
        Key::TipRectangle,
        Key::TipEllipse,
        Key::TipText,
        Key::TipCounter,
        Key::TipBlur,
        Key::TipPixelate,
        Key::TipStyle,
        Key::TipUndo,
        Key::TipRedo,
        Key::TipClear,
        Key::TipCopy,
        Key::TipCopyText,
        Key::TipSave,
        Key::TipClose,
        Key::TipRightClick,
    ]
    .into_iter()
    .map(|k| tr(k, lang).to_owned())
    .collect()
}

#[cfg(test)]
mod tests {
    use vixeeny_editor::{Modifiers, Point, Tool};

    use super::*;

    #[test]
    fn every_tip_of_the_toolbar_has_its_text() {
        use vixeeny_overlay::layout::{TIP_COUNT, Tip};
        for lang in [
            vixeeny_common::i18n::Lang::En,
            vixeeny_common::i18n::Lang::Fr,
        ] {
            let tips = toolbar_tips(lang);
            assert_eq!(tips.len(), TIP_COUNT);
            assert_eq!(Tip::RightClick as usize, TIP_COUNT - 1);
            assert!(tips.iter().all(|t| !t.is_empty()));
        }
    }

    #[test]
    fn a_saved_picture_is_shown_centred_where_it_fits() {
        // Two 1920×1080 monitors side by side, the cursor on the right one.
        let bounds = Rect::new(0.0, 0.0, 3840.0, 1080.0);
        let right = Rect::new(1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            place_picture((800, 600), &bounds, &right),
            Some((2480, 240))
        );
        assert_eq!(
            place_picture((1920, 1080), &bounds, &right),
            Some((1920, 0))
        );
        // Both screens at once: across the desktop.
        assert_eq!(place_picture((3840, 1080), &bounds, &right), Some((0, 0)));
        assert_eq!(
            place_picture((2000, 500), &bounds, &right),
            Some((920, 290))
        );
        assert_eq!(place_picture((4000, 500), &bounds, &right), None);
    }

    #[test]
    fn the_zone_of_a_saved_picture_stays_on_it() {
        let base = RgbaImage::new(300, 200, [32; 4].repeat(300 * 200)).unwrap();
        let picture = Rect::new(50.0, 40.0, 100.0, 80.0);
        let mut session = Session::new(base, Vec::new()).within(picture);
        assert_eq!(session.zone(), Some(picture));
        let none = Modifiers::default();
        // A new zone drawn past the picture is cut to it.
        session.pointer_down(Point::new(10.0, 10.0), none);
        session.pointer_move(Point::new(290.0, 190.0), none);
        session.pointer_up(Point::new(290.0, 190.0), none);
        assert_eq!(session.zone(), Some(picture));
        assert_eq!(session.export().unwrap().width, 100);
    }

    /// Text drawn by the editor is read back by the OCR engine of Windows (where a recognition
    /// language is installed).
    #[test]
    fn the_text_of_a_zone_is_read() {
        let (w, h) = (420, 80);
        let base = RgbaImage::new(w, h, [255; 4].repeat((w * h) as usize)).unwrap();
        let mut session = Session::new(base, Vec::new());
        session.select_all();
        session.choose_tool(Some(Tool::Text));
        session.set_color(vixeeny_editor::Color::rgb(0, 0, 0));
        let none = Modifiers::default();
        session.pointer_down(Point::new(20.0, 20.0), none);
        session.pointer_up(Point::new(20.0, 20.0), none);
        session.commit_text("Vixeeny reads 2026");
        let img = session.export().unwrap();
        let bgra: Vec<u8> = img
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| [b, g, r, a])
            .collect();
        match vixeeny_platform::ocr::recognize(w, h, &bgra) {
            Ok(text) => assert!(
                text.contains("Vixeeny") && text.contains("2026"),
                "{text:?}"
            ),
            Err(e) => vixeeny_common::skip_test(format!("no OCR: {e}")),
        }
    }

    #[test]
    fn annotations_keep_hdr_around_them_and_sit_at_sdr_white() {
        // A grey desktop shown tone-mapped, whose HDR is a 1000-nit highlight everywhere.
        let (w, h) = (200, 100);
        let base = RgbaImage::new(w, h, [128, 128, 128, 255].repeat((w * h) as usize)).unwrap();
        let desktop = HdrCapture {
            frame: HdrFrame {
                width: w,
                height: h,
                rgba: [12.5, 12.5, 12.5, 1.0].repeat((w * h) as usize),
            },
            info: HdrInfo {
                sdr_white_nits: 240.0,
                peak_nits: 1000.0,
            },
        };
        let mut session = Session::new(base, Vec::new());
        session.select_all();
        session.choose_tool(Some(Tool::Rect));
        session.set_filled(true);
        let none = Modifiers::default();
        session.pointer_down(Point::new(80.0, 40.0), none);
        session.pointer_move(Point::new(120.0, 60.0), none);
        session.pointer_up(Point::new(120.0, 60.0), none);
        let exported = session.export().unwrap();
        let hdr = annotated_hdr(&session, &exported, &desktop).unwrap();
        assert_eq!((hdr.frame.width, hdr.frame.height), (w, h));
        let px = |x: u32, y: u32| &hdr.frame.rgba[((y * w + x) * 4) as usize..][..4];
        // Untouched: the screen's light.
        assert_eq!(px(20, 10), &[12.5, 12.5, 12.5, 1.0]);
        // Under the rectangle: its colour, no brighter than SDR white (3.0 = 240 nits).
        let inside = px(100, 50);
        assert!(inside[..3].iter().all(|&c| c <= 3.0 + 1e-4), "{inside:?}");
        assert_ne!(inside, px(20, 10));
    }
}
