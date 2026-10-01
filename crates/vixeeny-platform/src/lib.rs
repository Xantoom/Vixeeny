// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-platform — OS abstractions: monitors, DPI, windows, processes (plan 4.3).
//!
//! All coordinates are **physical pixels** in the virtual desktop. Callers must have called
//! [`ensure_dpi_aware`] first, otherwise Windows hands out scaled (virtualised) values.

mod geometry;
mod time;
pub use geometry::{PhysicalRect, virtual_bounds};
pub use time::{LocalTime, local_time};

#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
use windows_impl as os;

#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
use unsupported as os;

pub use os::{
    cursor_position, ensure_dpi_aware, exclude_from_capture, foreground_window, hdr_info, monitors,
    top_level_windows, window_info,
};

/// Opaque monitor handle (an `HMONITOR` on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonitorId(pub u64);

/// Opaque window handle (an `HWND` on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: MonitorId,
    /// Device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Position and size in the virtual desktop, physical pixels.
    pub rect: PhysicalRect,
    pub primary: bool,
    /// Effective DPI (96 = 100 %).
    pub dpi: u32,
    /// `Some` while the monitor shows HDR content.
    pub hdr: Option<HdrInfo>,
}

impl MonitorInfo {
    pub fn scale_factor(&self) -> f64 {
        f64::from(self.dpi) / 96.0
    }
}

/// A monitor currently showing HDR content (Windows "Use HDR" on).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrInfo {
    /// Brightness of SDR white in nits (the "SDR content brightness" slider; 80 at its minimum).
    pub sdr_white_nits: f32,
    /// Peak brightness reported by the display, in nits.
    pub peak_nits: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    /// Visible frame (without the invisible resize border on Windows 10/11).
    pub rect: PhysicalRect,
    pub pid: u32,
    /// Full path of the executable, when readable.
    pub exe_path: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("not supported on this platform yet")]
    Unsupported,
    #[error("{0}")]
    Os(String),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

/// The monitor containing `point`, or the closest one (like `MONITOR_DEFAULTTONEAREST`).
pub fn monitor_at(monitors: &[MonitorInfo], x: i32, y: i32) -> Option<&MonitorInfo> {
    monitors
        .iter()
        .find(|m| m.rect.contains(x, y))
        .or_else(|| monitors.iter().min_by_key(|m| m.rect.distance2(x, y)))
}

/// The monitor holding most of `rect` (ties: first).
pub fn monitor_for_rect<'a>(
    monitors: &'a [MonitorInfo],
    rect: &PhysicalRect,
) -> Option<&'a MonitorInfo> {
    monitors
        .iter()
        .max_by_key(|m| m.rect.intersection(rect).map_or(0, |r| r.area()))
        .filter(|m| m.rect.intersection(rect).is_some())
        .or_else(|| monitors.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: u64, x: i32, y: i32, w: u32, h: u32, dpi: u32) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId(id),
            name: format!("DISPLAY{id}"),
            rect: PhysicalRect::new(x, y, w, h),
            primary: id == 1,
            dpi,
            hdr: None,
        }
    }

    /// 4K at 150 % left of a 1080p at 100 % (CA-IMG-3 layout).
    fn layout() -> Vec<MonitorInfo> {
        vec![
            mon(1, 0, 0, 3840, 2160, 144),
            mon(2, 3840, 0, 1920, 1080, 96),
        ]
    }

    #[test]
    fn point_lookup() {
        let m = layout();
        assert_eq!(monitor_at(&m, 10, 10).unwrap().id, MonitorId(1));
        assert_eq!(monitor_at(&m, 4000, 500).unwrap().id, MonitorId(2));
        // Below the 1080p monitor but only 160 px right of the 4K one's edge: the 4K is nearer.
        assert_eq!(monitor_at(&m, 4000, 1500).unwrap().id, MonitorId(1));
        assert_eq!(monitor_at(&m, 5000, 1100).unwrap().id, MonitorId(2));
        assert!(monitor_at(&[], 0, 0).is_none());
    }

    #[test]
    fn rect_lookup_picks_largest_overlap() {
        let m = layout();
        let straddling = PhysicalRect::new(3600, 100, 600, 300); // 240 px left, 360 px right
        assert_eq!(monitor_for_rect(&m, &straddling).unwrap().id, MonitorId(2));
    }

    #[test]
    fn scale_factor() {
        assert!((layout()[0].scale_factor() - 1.5).abs() < f64::EPSILON);
    }
}
