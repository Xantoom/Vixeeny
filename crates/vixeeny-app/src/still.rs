// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct image captures (plan 5.2): full screen under the cursor, active window, all
//! monitors. Written in the configured format in the images folder. The editor flow (`capture-region`) is M7.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vixeeny_capture::{CaptureError, CaptureOptions, CaptureTarget, Capturer, StillBackend};
use vixeeny_common::ipc::ActionId;
use vixeeny_common::naming::{self, AppInfo, Vars};
use vixeeny_image::{Bgra, ImageError, ImageFormat, Settings};
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

/// Where and under which name captures are written (plan 5.8).
pub struct Destination<'a> {
    pub dir: &'a Path,
    pub template: &'a str,
    pub per_app_subfolder: bool,
    /// `{app}` of a desktop capture is the full-screen application with the focus (a game),
    /// not `Desktop`.
    pub use_foreground_app: bool,
    pub app_names: &'a BTreeMap<String, String>,
    pub now: &'a LocalTime,
    /// Called with the image once it is written (e.g. to copy it to the clipboard).
    pub after_save: Option<&'a dyn Fn(&Bgra<'_>)>,
}

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

/// Programs that fill the screen without being a game: the desktop, the lock screen, the
/// browsers and the video players (a film in full screen is not worth a replay).
const NOT_GAMES: &[&str] = &[
    "explorer.exe",
    "lockapp.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "applicationframehost.exe",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "opera.exe",
    "brave.exe",
    "vivaldi.exe",
    "vlc.exe",
    "mpc-hc64.exe",
    "mpc-be64.exe",
    "mpv.exe",
    "potplayermini64.exe",
];

/// The full-screen game in the foreground, if there is one: a window of another program that
/// covers its whole monitor (borderless full screen too).
pub fn game_in_front() -> Option<WindowInfo> {
    let window = vixeeny_platform::foreground_window().ok()??;
    let monitors = vixeeny_platform::monitors().ok()?;
    let exe = window
        .exe_path
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().to_ascii_lowercase())?;
    (is_full_screen(&window, &monitors) && !is_ours(&window) && !NOT_GAMES.contains(&exe.as_str()))
        .then_some(window)
}

/// Whether `window` belongs to Vixeeny: this process, or an executable next to it (the daemon).
fn is_ours(window: &WindowInfo) -> bool {
    if window.pid == std::process::id() {
        return true;
    }
    let dir = |p: &Path| p.parent().map(|d| d.to_string_lossy().to_lowercase());
    match (window.exe_path.as_deref(), std::env::current_exe()) {
        (Some(exe), Ok(me)) => dir(Path::new(exe)).is_some() && dir(Path::new(exe)) == dir(&me),
        _ => false,
    }
}

/// A window covering a whole monitor: a game or a video, not a window among others.
fn is_full_screen(window: &WindowInfo, monitors: &[MonitorInfo]) -> bool {
    let w = &window.rect;
    monitors.iter().any(|m| {
        let m = &m.rect;
        w.x <= m.x
            && w.y <= m.y
            && w.x + w.width as i32 >= m.x + m.width as i32
            && w.y + w.height as i32 >= m.y + m.height as i32
    })
}

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
    output: (ImageFormat, &Settings),
    metadata: &dyn Fn(&str) -> ExeMetadata,
) -> Result<PathBuf, StillError> {
    let target = target_for(action, snap)?;
    let started = std::time::Instant::now();
    let frame = capturer.grab(&target, options)?;
    let grabbed = started.elapsed();
    let image = Bgra::new(frame.width, frame.height, frame.stride, &frame.data);
    let path = save_image(action, snap, dest, output, metadata, &image)?;
    tracing::debug!(
        "grabbed in {grabbed:?}, encoded and written in {:?}",
        started.elapsed() - grabbed
    );
    Ok(path)
}

/// Encodes `image` and writes it under the configured name. Shared by the direct captures and
/// the editor's "save".
pub fn save_image(
    action: ActionId,
    snap: &Snapshot,
    dest: &Destination<'_>,
    output: (ImageFormat, &Settings),
    metadata: &dyn Fn(&str) -> ExeMetadata,
    image: &Bgra<'_>,
) -> Result<PathBuf, StillError> {
    let (format, settings) = output;
    let bytes = vixeeny_image::encode(format, image, settings)?;
    let vars = template_vars(action, snap, dest, metadata, (image.width, image.height));
    let path = naming::output_path(
        dest.dir,
        dest.per_app_subfolder,
        dest.template,
        &vars,
        format.extension(),
        std::path::Path::exists,
    );
    write_atomic(&path, &bytes)?;
    if let Some(after) = dest.after_save {
        after(image);
    }
    Ok(path)
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

    #[test]
    fn other_actions_are_refused() {
        assert!(matches!(
            target_for(ActionId::RecordToggle, &snapshot()),
            Err(StillError::NotADirectCapture(_))
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
            (ImageFormat::Png, &Settings::default()),
            metadata,
        )
        .unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vixeeny-still-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
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
}
