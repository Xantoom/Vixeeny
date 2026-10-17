// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct image captures: full screen under the cursor, active window, all
//! monitors. Written in the configured format in the images folder. The editor flow (`capture-region`) is M7.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vixeeny_capture::{
    CaptureError, CaptureOptions, CaptureTarget, Capturer, HdrCapture, HdrFrame, StillBackend,
};
use vixeeny_common::ipc::ActionId;
use vixeeny_common::naming::{self, AppInfo, Vars};
use vixeeny_image::backdrop::{self, Backdrop, Linear};
use vixeeny_image::{Bgra, Hdr, ImageError, ImageFormat, Settings};
use vixeeny_overlay::backdrop as card;
use vixeeny_platform::{ExeMetadata, LocalTime, MonitorInfo, WindowInfo, monitor_at};

#[derive(Debug, thiserror::Error)]
pub enum StillError {
    #[error("`{0:?}` is not a direct capture")]
    NotADirectCapture(ActionId),
    #[error("no monitor found")]
    NoMonitor,
    #[error("no window has the focus")]
    NoWindow,
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Image(#[from] ImageError),
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// What the OS says at the moment of the shortcut.
#[derive(Clone)]
pub struct Snapshot {
    pub monitors: Vec<MonitorInfo>,
    pub cursor: (i32, i32),
    pub foreground: Option<WindowInfo>,
}

pub fn target_for(action: ActionId, snap: &Snapshot) -> Result<CaptureTarget, StillError> {
    match action {
        ActionId::CaptureFullscreen => {
            let (x, y) = snap.cursor;
            let monitor = monitor_at(&snap.monitors, x, y).ok_or(StillError::NoMonitor)?;
            Ok(CaptureTarget::Monitor(monitor.id))
        }
        ActionId::CaptureWindow => snap
            .foreground
            .as_ref()
            .map(|w| CaptureTarget::Window(w.id))
            .ok_or(StillError::NoWindow),
        ActionId::CaptureAllMonitors => Ok(CaptureTarget::AllMonitors),
        other => Err(StillError::NotADirectCapture(other)),
    }
}

/// Writes next to the destination then renames, so a crash never leaves a truncated file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StillError> {
    let io = |source| StillError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

/// How captures are encoded.
#[derive(Debug, Clone, Copy)]
pub struct Output<'a> {
    pub format: ImageFormat,
    pub settings: &'a Settings,
    /// HDR screens are kept as HDR when the format can hold it, rather than tone-mapped.
    pub keep_hdr: bool,
    /// A window capture is set on this background.
    pub backdrop: Option<Backdrop>,
}

impl Output<'_> {
    /// Whether an HDR capture is written as HDR.
    pub fn hdr(&self) -> bool {
        self.keep_hdr && self.format.holds_hdr()
    }
}

/// Where and under which name captures are written.
pub struct Destination<'a> {
    pub dir: &'a Path,
    pub template: &'a str,
    pub per_app_subfolder: bool,
    /// `{app}` of a desktop capture is the full-screen application with the focus (a game),
    /// not `Desktop`.
    pub use_foreground_app: bool,
    pub app_names: &'a BTreeMap<String, String>,
    pub now: &'a LocalTime,
    /// Called with the image once it is written (e.g. to copy it to the clipboard), and with
    /// the file's bytes when they are the image itself (not an HDR file of an SDR copy), so that
    /// it is not encoded again.
    pub after_save: Option<&'a AfterSave<'a>>,
}

/// What [`Destination::after_save`] calls: the image, and the file's bytes when they are it.
pub type AfterSave<'a> = dyn Fn(&Bgra<'_>, Option<&[u8]>) + 'a;

/// Display name of the application a capture is attributed to.
pub(crate) fn app_name(
    action: ActionId,
    snap: &Snapshot,
    dest: &Destination<'_>,
    metadata: &dyn Fn(&str) -> ExeMetadata,
) -> String {
    let window = match action {
        ActionId::CaptureWindow => snap.foreground.as_ref(),
        // Only a window over the screen being captured (all of them for an all-monitor
        // capture), and never one of Vixeeny's own (a menu, the settings, a notification).
        _ => snap.foreground.as_ref().filter(|w| {
            let monitors: Vec<MonitorInfo> = if action == ActionId::CaptureAllMonitors {
                snap.monitors.clone()
            } else {
                let (x, y) = snap.cursor;
                monitor_at(&snap.monitors, x, y)
                    .into_iter()
                    .cloned()
                    .collect()
            };
            dest.use_foreground_app && is_full_screen(w, &monitors) && !is_ours(w)
        }),
    };
    let resolved = window.and_then(|w| {
        let meta = w.exe_path.as_deref().map(metadata).unwrap_or_default();
        naming::resolve_app_name(
            &AppInfo {
                exe_path: w.exe_path.as_deref(),
                product_name: meta.product_name.as_deref(),
                file_description: meta.file_description.as_deref(),
                window_title: Some(&w.title),
            },
            dest.app_names,
        )
    });
    resolved.unwrap_or_else(|| {
        if action == ActionId::CaptureWindow {
            naming::DEFAULT_APP_NAME.to_owned()
        } else {
            naming::DESKTOP_APP_NAME.to_owned()
        }
    })
}

pub use vixeeny_platform::games::{is_full_screen, is_ours};

fn template_vars(
    action: ActionId,
    snap: &Snapshot,
    dest: &Destination<'_>,
    metadata: &dyn Fn(&str) -> ExeMetadata,
    size: (u32, u32),
) -> Vars {
    let monitor = match target_for(action, snap) {
        Ok(CaptureTarget::Monitor(id)) => snap
            .monitors
            .iter()
            .position(|m| m.id == id)
            .map_or_else(String::new, |i| (i + 1).to_string()),
        _ => String::new(),
    };
    Vars {
        app: app_name(action, snap, dest, metadata),
        title: snap
            .foreground
            .as_ref()
            .filter(|_| action == ActionId::CaptureWindow)
            .map(|w| naming::sanitize(&w.title))
            .unwrap_or_default(),
        date: dest.now.date(),
        time: dest.now.time(),
        millis: dest.now.millisecond,
        width: size.0,
        height: size.1,
        monitor,
    }
}

/// Grabs, encodes and saves. Returns the file written. `metadata` reads the version resource
/// of an executable (a parameter so tests need no real programs).
pub fn run<B: StillBackend>(
    action: ActionId,
    snap: &Snapshot,
    capturer: &mut Capturer<B>,
    dest: &Destination<'_>,
    options: CaptureOptions,
    output: Output<'_>,
    metadata: &dyn Fn(&str) -> ExeMetadata,
) -> Result<PathBuf, StillError> {
    let target = target_for(action, snap)?;
    let started = std::time::Instant::now();
    let hdr = if output.hdr() {
        capturer.grab_hdr(&target, options.show_cursor)?
    } else {
        None
    };
    // A window set on its background: the interface scale of its screen sizes the margin. It is
    // drawn on the capture's own graphics device.
    let device = capturer.d3d_device();
    let backdrop = output
        .backdrop
        .filter(|_| action == ActionId::CaptureWindow)
        .map(|b| {
            let scale = snap
                .foreground
                .as_ref()
                .and_then(|w| {
                    monitor_at(
                        &snap.monitors,
                        w.rect.x + w.rect.width as i32 / 2,
                        w.rect.y + w.rect.height as i32 / 2,
                    )
                })
                .map_or(1.0, |m| m.scale_factor() as f32);
            (b, scale)
        });
    let path = if let Some(mut capture) = hdr {
        let grabbed = started.elapsed();
        if let Some((b, scale)) = backdrop {
            let (w, h) = (capture.frame.width, capture.frame.height);
            let white = capture.info.sdr_white_nits / 80.0;
            let layout = backdrop::layout(w, h, scale);
            let window = card::Window::Hdr {
                width: w,
                height: h,
                rgba: &capture.frame.rgba,
            };
            capture.frame =
                match card::set_on_card(device.as_ref(), &window, &card_setting(&layout, b, white))
                {
                    Ok(card::Set::Hdr(rgba)) => HdrFrame {
                        width: layout.size.0,
                        height: layout.size.1,
                        rgba,
                    },
                    card => {
                        if let Err(e) = card {
                            tracing::debug!("backdrop on the processor: {e}");
                        }
                        let window = Linear {
                            width: w,
                            height: h,
                            rgba: std::mem::take(&mut capture.frame.rgba),
                        };
                        let set = backdrop::set_on(&window, b, scale, white);
                        HdrFrame {
                            width: set.width,
                            height: set.height,
                            rgba: set.rgba,
                        }
                    }
                };
        }
        let path = save_hdr(action, snap, dest, output, metadata, &capture)?;
        tracing::debug!(
            "grabbed (HDR) in {grabbed:?}, encoded and written in {:?}",
            started.elapsed() - grabbed
        );
        path
    } else {
        let frame = capturer.grab(&target, options)?;
        let grabbed = started.elapsed();
        let set = backdrop.map(|(b, scale)| {
            let layout = backdrop::layout(frame.width, frame.height, scale);
            let window = card::Window::Sdr {
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
                bgra: &frame.data,
            };
            match card::set_on_card(device.as_ref(), &window, &card_setting(&layout, b, 1.0)) {
                Ok(card::Set::Sdr(bgra)) => (layout.size.0, layout.size.1, bgra),
                card => {
                    if let Err(e) = card {
                        tracing::debug!("backdrop on the processor: {e}");
                    }
                    let window = backdrop::linear_from_bgra(
                        frame.width,
                        frame.height,
                        frame.stride,
                        &frame.data,
                    );
                    let set = backdrop::set_on(&window, b, scale, 1.0);
                    (set.width, set.height, backdrop::linear_to_bgra(&set))
                }
            }
        });
        let image = match &set {
            Some((w, h, bgra)) => Bgra::new(*w, *h, *w as usize * 4, bgra),
            None => Bgra::new(frame.width, frame.height, frame.stride, &frame.data),
        };
        let path = save_image(action, snap, dest, output, metadata, &image)?;
        tracing::debug!(
            "grabbed in {grabbed:?}, encoded and written in {:?}",
            started.elapsed() - grabbed
        );
        path
    };
    Ok(path)
}

/// Encodes `image` and writes it under the configured name. Shared by the direct captures and
/// the editor's "save".
pub fn save_image(
    action: ActionId,
    snap: &Snapshot,
    dest: &Destination<'_>,
    output: Output<'_>,
    metadata: &dyn Fn(&str) -> ExeMetadata,
    image: &Bgra<'_>,
) -> Result<PathBuf, StillError> {
    let bytes = vixeeny_image::encode(output.format, image, output.settings)?;
    let path = write_named(
        action,
        snap,
        dest,
        output.format,
        metadata,
        (image.width, image.height),
        &bytes,
    )?;
    if let Some(after) = dest.after_save {
        after(image, Some(&bytes));
    }
    Ok(path)
}

/// [`save_image`] for an HDR capture, kept as HDR. What comes after the save (the clipboard)
/// gets the SDR rendering.
pub fn save_hdr(
    action: ActionId,
    snap: &Snapshot,
    dest: &Destination<'_>,
    output: Output<'_>,
    metadata: &dyn Fn(&str) -> ExeMetadata,
    capture: &HdrCapture,
) -> Result<PathBuf, StillError> {
    let image = hdr_image(capture);
    let bytes = vixeeny_image::encode_hdr(output.format, &image, output.settings)?;
    let size = (image.width, image.height);
    let path = write_named(action, snap, dest, output.format, metadata, size, &bytes)?;
    if let Some(after) = dest.after_save {
        let sdr = image.to_sdr();
        after(&Bgra::new(size.0, size.1, size.0 as usize * 4, &sdr), None);
    }
    Ok(path)
}

pub fn hdr_image(capture: &HdrCapture) -> Hdr<'_> {
    Hdr {
        width: capture.frame.width,
        height: capture.frame.height,
        rgba: &capture.frame.rgba,
        sdr_white_nits: capture.info.sdr_white_nits,
        peak_nits: capture.info.peak_nits,
    }
}

fn write_named(
    action: ActionId,
    snap: &Snapshot,
    dest: &Destination<'_>,
    format: ImageFormat,
    metadata: &dyn Fn(&str) -> ExeMetadata,
    size: (u32, u32),
    bytes: &[u8],
) -> Result<PathBuf, StillError> {
    let vars = template_vars(action, snap, dest, metadata, size);
    let path = naming::output_path(
        dest.dir,
        dest.per_app_subfolder,
        dest.template,
        &vars,
        format.extension(),
        std::path::Path::exists,
    );
    write_atomic(&path, bytes)?;
    Ok(path)
}

/// The graphics card's version of `b`, with the processor's sizes.
fn card_setting(layout: &backdrop::Layout, b: Backdrop, white: f32) -> card::Setting {
    card::Setting {
        margin: layout.margin,
        radius: layout.radius,
        offset: layout.offset,
        softness: layout.softness,
        small: layout.small,
        small_radius: layout.small_radius,
        darken: backdrop::DARKEN,
        shadow: backdrop::SHADOW,
        behind: match b {
            Backdrop::Blur => card::Behind::Blur,
            Backdrop::Colour(c) => card::Behind::Colour(backdrop::linear_colour(c, white)),
        },
    }
}

#[cfg(test)]
mod tests {
    use vixeeny_capture::FakeBackend;
    use vixeeny_platform::{MonitorId, PhysicalRect, WindowId};

    use super::*;

    fn mon(id: u64, x: i32, w: u32) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId(id),
            name: format!("D{id}"),
            rect: PhysicalRect::new(x, 0, w, 100),
            work: PhysicalRect::new(x, 0, w, 100),
            primary: id == 1,
            dpi: 96,
            hdr: None,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            monitors: vec![mon(1, 0, 200), mon(2, 200, 160)],
            cursor: (250, 10),
            foreground: Some(WindowInfo {
                id: WindowId(77),
                title: "Game".into(),
                rect: PhysicalRect::new(0, 0, 10, 10),
                pid: 1,
                exe_path: None,
            }),
        }
    }

    fn now() -> LocalTime {
        LocalTime::from_unix_utc(1_790_874_720, 0)
    }

    #[test]
    fn full_screen_follows_the_cursor() {
        assert_eq!(
            target_for(ActionId::CaptureFullscreen, &snapshot()).unwrap(),
            CaptureTarget::Monitor(MonitorId(2))
        );
    }

    #[test]
    fn window_needs_a_foreground_window() {
        let mut s = snapshot();
        assert_eq!(
            target_for(ActionId::CaptureWindow, &s).unwrap(),
            CaptureTarget::Window(WindowId(77))
        );
        s.foreground = None;
        assert!(matches!(
            target_for(ActionId::CaptureWindow, &s),
            Err(StillError::NoWindow)
        ));
    }

    fn no_metadata(_: &str) -> ExeMetadata {
        ExeMetadata::default()
    }

    #[allow(clippy::too_many_arguments)]
    fn run_with(
        action: ActionId,
        snap: &Snapshot,
        dir: &Path,
        template: &str,
        per_app: bool,
        use_fg: bool,
        names: &BTreeMap<String, String>,
        metadata: &dyn Fn(&str) -> ExeMetadata,
    ) -> PathBuf {
        let mut capturer = Capturer::new(FakeBackend::default(), snap.monitors.clone());
        let now = now();
        let dest = Destination {
            dir,
            template,
            per_app_subfolder: per_app,
            use_foreground_app: use_fg,
            app_names: names,
            now: &now,
            after_save: None,
        };
        run(
            action,
            snap,
            &mut capturer,
            &dest,
            CaptureOptions::default(),
            Output {
                format: ImageFormat::Png,
                settings: &Settings::default(),
                keep_hdr: false,
                backdrop: None,
            },
            metadata,
        )
        .unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vixeeny-still-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A window capture on its background: larger by the margin, the background in the corner;
    /// other captures untouched.
    #[test]
    fn a_window_is_set_on_its_background() {
        let dir = temp_dir("backdrop");
        let snap = snapshot();
        let names = BTreeMap::new();
        let now = now();
        let dest = Destination {
            dir: &dir,
            template: "{app}_{time}",
            per_app_subfolder: false,
            use_foreground_app: false,
            app_names: &names,
            now: &now,
            after_save: None,
        };
        let shoot = |action| {
            let mut capturer = Capturer::new(FakeBackend::default(), snap.monitors.clone());
            let path = run(
                action,
                &snap,
                &mut capturer,
                &dest,
                CaptureOptions::default(),
                Output {
                    format: ImageFormat::Png,
                    settings: &Settings::default(),
                    keep_hdr: false,
                    backdrop: Some(Backdrop::Colour([0, 120, 212])),
                },
                &no_metadata,
            )
            .unwrap();
            vixeeny_image::decode(&std::fs::read(path).unwrap()).unwrap()
        };
        let window = shoot(ActionId::CaptureWindow);
        // 64×48 at 100 %: 48 pixels all around.
        assert_eq!((window.width, window.height), (64 + 96, 48 + 96));
        assert_eq!(&window.bgra[..4], &[212, 120, 0, 255]);
        let screen = shoot(ActionId::CaptureFullscreen);
        assert_eq!((screen.width, screen.height), (160, 100));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn writes_a_decodable_png_without_clobbering() {
        let dir = temp_dir("png");
        let snap = Snapshot {
            foreground: None,
            ..snapshot()
        };
        let names = BTreeMap::new();
        let tpl = "{app}_{date}_{time}";
        let first = run_with(
            ActionId::CaptureFullscreen,
            &snap,
            &dir,
            tpl,
            false,
            true,
            &names,
            &no_metadata,
        );
        let second = run_with(
            ActionId::CaptureFullscreen,
            &snap,
            &dir,
            tpl,
            false,
            true,
            &names,
            &no_metadata,
        );
        assert_eq!(
            first.file_name().unwrap(),
            "Desktop_2026-10-01_17-12-00.png"
        );
        assert_eq!(
            second.file_name().unwrap(),
            "Desktop_2026-10-01_17-12-00_2.png"
        );
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&first).unwrap(),
        ));
        let reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (160, 100));
        assert!(!dir.join("Desktop_2026-10-01_17-12-00.png.part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn per_app_folder_uses_the_resolved_name() {
        let dir = temp_dir("app");
        let mut snap = snapshot();
        snap.foreground.as_mut().unwrap().exe_path = Some(r"C:\G\Client-Win64-Shipping.exe".into());
        let names: BTreeMap<_, _> = [(
            "client-win64-shipping.exe".to_owned(),
            "Wuthering Waves".to_owned(),
        )]
        .into();
        let path = run_with(
            ActionId::CaptureWindow,
            &snap,
            &dir,
            "{app}_{date}_{time}",
            true,
            true,
            &names,
            &no_metadata,
        );
        assert_eq!(
            path,
            dir.join("Wuthering Waves")
                .join("Wuthering Waves_2026-10-01_17-12-00.png")
        );
        // Product name from the version resource wins when the table has no entry.
        let meta = |_: &str| ExeMetadata {
            product_name: Some("Wuthering: Waves?".into()),
            file_description: None,
        };
        let path = run_with(
            ActionId::CaptureWindow,
            &snap,
            &dir,
            "{app}-{width}x{height}",
            true,
            true,
            &BTreeMap::new(),
            &meta,
        );
        assert_eq!(
            path,
            dir.join("Wuthering Waves")
                .join("Wuthering Waves-64x48.png")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desktop_captures_follow_the_foreground_app_setting() {
        let dir = temp_dir("fg");
        // A window among others: the capture is the desktop's.
        let names = BTreeMap::new();
        let windowed = run_with(
            ActionId::CaptureFullscreen,
            &snapshot(),
            &dir,
            "{app}",
            false,
            true,
            &names,
            &no_metadata,
        );
        assert_eq!(windowed.file_name().unwrap(), "Desktop.png");
        let _ = std::fs::remove_file(&windowed);
        // A full-screen game (titled "Game", unknown exe) names it.
        let mut snap = snapshot();
        snap.foreground.as_mut().unwrap().rect = PhysicalRect::new(200, 0, 160, 100);
        let on = run_with(
            ActionId::CaptureFullscreen,
            &snap,
            &dir,
            "{app}",
            false,
            true,
            &names,
            &no_metadata,
        );
        assert_eq!(on.file_name().unwrap(), "Game.png");
        let _ = std::fs::remove_file(&on);
        let off = run_with(
            ActionId::CaptureFullscreen,
            &snap,
            &dir,
            "{app}",
            false,
            false,
            &names,
            &no_metadata,
        );
        assert_eq!(off.file_name().unwrap(), "Desktop.png");
        let _ = std::fs::remove_file(&off);
        // Vixeeny's own window over the screen (a menu, the settings) is not a game.
        let mut ours = snap.clone();
        ours.foreground.as_mut().unwrap().pid = std::process::id();
        let own = run_with(
            ActionId::CaptureFullscreen,
            &ours,
            &dir,
            "{app}",
            false,
            true,
            &names,
            &no_metadata,
        );
        assert_eq!(own.file_name().unwrap(), "Desktop.png");
        let _ = std::fs::remove_file(&own);
        // A game on the other screen names neither this screen's capture...
        let mut other = snapshot();
        other.foreground.as_mut().unwrap().rect = PhysicalRect::new(0, 0, 200, 100);
        let here = run_with(
            ActionId::CaptureFullscreen,
            &other,
            &dir,
            "{app}",
            false,
            true,
            &names,
            &no_metadata,
        );
        assert_eq!(here.file_name().unwrap(), "Desktop.png");
        // ...but it does name a capture of every screen.
        let all = run_with(
            ActionId::CaptureAllMonitors,
            &other,
            &dir,
            "{app}",
            false,
            true,
            &names,
            &no_metadata,
        );
        assert_eq!(all.file_name().unwrap(), "Game.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_hdr_screen_is_kept_as_hdr_when_asked_and_the_format_can() {
        let dir = temp_dir("hdr");
        let mut snap = snapshot();
        snap.foreground = None;
        snap.monitors[1].hdr = Some(vixeeny_platform::HdrInfo {
            sdr_white_nits: 200.0,
            peak_nits: 1000.0,
        });
        let now = now();
        let names = BTreeMap::new();
        let copied = std::cell::Cell::new(None);
        // The file's bytes, when the clipboard can take them as they are.
        let encoded = std::cell::RefCell::new(None);
        let copy = |image: &Bgra<'_>, bytes: Option<&[u8]>| {
            copied.set(Some((image.width, image.data[0])));
            *encoded.borrow_mut() = bytes.map(<[u8]>::to_vec);
        };
        let dest = Destination {
            dir: &dir,
            template: "{app}",
            per_app_subfolder: false,
            use_foreground_app: true,
            app_names: &names,
            now: &now,
            after_save: Some(&copy),
        };
        let settings = Settings::default();
        let options = CaptureOptions {
            tonemap: Some(|rgba, _| vec![7; rgba.len()]),
            ..CaptureOptions::default()
        };
        let shoot = |format, keep_hdr| {
            let mut capturer = Capturer::new(FakeBackend::default(), snap.monitors.clone());
            let output = Output {
                format,
                settings: &settings,
                keep_hdr,
                backdrop: None,
            };
            let path = run(
                ActionId::CaptureFullscreen,
                &snap,
                &mut capturer,
                &dest,
                options,
                output,
                &no_metadata,
            )
            .unwrap();
            std::fs::read(path).unwrap()
        };
        // Kept: a PQ PNG; the clipboard still gets the SDR rendering (the fake's 2.0 = 160
        // nits, under SDR white: mid grey, not the fake tone mapper's 7).
        let png = shoot(ImageFormat::Png, true);
        assert!(png.windows(4).any(|c| c == b"cICP"));
        let (width, first) = copied.take().unwrap();
        assert_eq!(width, 160);
        assert!((150..250).contains(&first), "{first}");
        // An HDR file is not the clipboard's SDR image: that one is encoded on its own.
        assert!(encoded.take().is_none());
        // Converted when asked, and always for WebP.
        let sdr = shoot(ImageFormat::Png, false);
        assert!(!sdr.windows(4).any(|c| c == b"cICP"));
        assert_eq!(copied.take().unwrap().1, 7);
        // The SDR file is the clipboard's image: not encoded a second time.
        assert_eq!(encoded.take(), Some(sdr));
        let webp = shoot(ImageFormat::WebP, true);
        assert_eq!(&webp[8..12], b"WEBP");
        assert_eq!(copied.take().unwrap().1, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
