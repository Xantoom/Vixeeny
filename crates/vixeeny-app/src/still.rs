// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct image captures (plan 5.2): full screen under the cursor, active window, all
//! monitors. Written in the configured format in the images folder. The editor flow (`capture-region`) is M7.

use std::path::{Path, PathBuf};

use vixeeny_capture::{CaptureError, CaptureOptions, CaptureTarget, Capturer, StillBackend};
use vixeeny_common::ipc::ActionId;
use vixeeny_image::{Bgra, ImageError, ImageFormat, Settings};
use vixeeny_platform::{LocalTime, MonitorInfo, WindowInfo, monitor_at};

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

/// `Vixeeny_2026-10-01_17-12-00.png`, then `…_2.png`, `…_3.png` if it already exists. The
/// configurable template and per-application folders arrive with M6.
pub fn unique_path(dir: &Path, now: &LocalTime, ext: &str) -> PathBuf {
    let stem = format!("Vixeeny_{}_{}", now.date(), now.time());
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}_{n}.{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
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

/// Grabs, encodes and saves. Returns the file written.
pub fn run<B: StillBackend>(
    action: ActionId,
    snap: &Snapshot,
    capturer: &mut Capturer<B>,
    images_dir: &Path,
    options: CaptureOptions,
    output: (ImageFormat, &Settings),
    now: &LocalTime,
) -> Result<PathBuf, StillError> {
    let target = target_for(action, snap)?;
    let frame = capturer.grab(&target, options)?;
    let (format, settings) = output;
    let image = Bgra::new(frame.width, frame.height, frame.stride, &frame.data);
    let bytes = vixeeny_image::encode(format, &image, settings)?;
    let path = unique_path(images_dir, now, format.extension());
    write_atomic(&path, &bytes)?;
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
            primary: id == 1,
            dpi: 96,
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

    #[test]
    fn writes_a_decodable_png_without_clobbering() {
        let dir = std::env::temp_dir().join(format!("vixeeny-still-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let snap = snapshot();
        let mut capturer = Capturer::new(FakeBackend::default(), snap.monitors.clone());
        let run_once = |capturer: &mut Capturer<FakeBackend>| {
            run(
                ActionId::CaptureFullscreen,
                &snap,
                capturer,
                &dir,
                CaptureOptions::default(),
                (ImageFormat::Png, &Settings::default()),
                &now(),
            )
            .unwrap()
        };
        let first = run_once(&mut capturer);
        let second = run_once(&mut capturer);
        assert_eq!(
            first.file_name().unwrap(),
            "Vixeeny_2026-10-01_17-12-00.png"
        );
        assert_eq!(
            second.file_name().unwrap(),
            "Vixeeny_2026-10-01_17-12-00_2.png"
        );
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&first).unwrap(),
        ));
        let reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (160, 100));
        assert!(!dir.join("Vixeeny_2026-10-01_17-12-00.png.part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
