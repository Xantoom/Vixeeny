// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS: displays, cursor and the screen-recording permission through CoreGraphics. Positions
//! are physical pixels like on Windows: a display's bounds (points) times its backing scale.
//!
//! What is not here yet falls back to [`crate::unsupported`] (windows list, HDR, …).

use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayPixelsHigh, CGDisplayPixelsWide, CGEvent, CGGetActiveDisplayList,
    CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};

pub use crate::unsupported::*;
use crate::{MonitorId, MonitorInfo, PhysicalRect, PlatformError, Result};

const MAX_DISPLAYS: usize = 16;

fn display_ids() -> Result<Vec<u32>> {
    let mut ids = [0u32; MAX_DISPLAYS];
    let mut count = 0u32;
    // SAFETY: `ids` has room for MAX_DISPLAYS entries and `count` receives how many are set.
    let status =
        unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &raw mut count) };
    if status.0 != 0 {
        return Err(PlatformError::Os(format!(
            "CGGetActiveDisplayList: {}",
            status.0
        )));
    }
    Ok(ids[..count as usize].to_vec())
}

/// One display: its bounds in points and its size in pixels give the backing scale.
fn describe(id: u32, primary: u32) -> MonitorInfo {
    let bounds = CGDisplayBounds(id);
    let pixels_w = CGDisplayPixelsWide(id) as f64;
    let scale = if bounds.size.width > 0.0 {
        (pixels_w / bounds.size.width).max(1.0)
    } else {
        1.0
    };
    let pixels_h = CGDisplayPixelsHigh(id) as f64;
    MonitorInfo {
        id: MonitorId(u64::from(id)),
        name: format!("Display {id}"),
        rect: PhysicalRect::new(
            (bounds.origin.x * scale).round() as i32,
            (bounds.origin.y * scale).round() as i32,
            pixels_w.round() as u32,
            pixels_h.round() as u32,
        ),
        primary: id == primary,
        dpi: (96.0 * scale).round() as u32,
        hdr: None,
    }
}

pub fn monitors() -> Result<Vec<MonitorInfo>> {
    let primary = CGMainDisplayID();
    Ok(display_ids()?
        .into_iter()
        .map(|id| describe(id, primary))
        .collect())
}

/// The pointer, in the same physical coordinates as [`monitors`].
pub fn cursor_position() -> Result<(i32, i32)> {
    let event =
        CGEvent::new(None).ok_or_else(|| PlatformError::Os("cannot read the pointer".into()))?;
    let at = CGEvent::location(Some(&event));
    let primary = CGMainDisplayID();
    // Points → pixels with the scale of the display under the pointer.
    let scale = display_ids()?
        .into_iter()
        .map(|id| (describe(id, primary), CGDisplayBounds(id)))
        .find(|(_, b)| {
            at.x >= b.origin.x
                && at.x < b.origin.x + b.size.width
                && at.y >= b.origin.y
                && at.y < b.origin.y + b.size.height
        })
        .map_or(1.0, |(m, _)| f64::from(m.dpi) / 96.0);
    Ok(((at.x * scale).round() as i32, (at.y * scale).round() as i32))
}

/// Whether the app may record the screen (System Settings → Privacy → Screen Recording).
pub fn screen_capture_allowed() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Asks for the permission: the first call shows the system prompt. `true` when already granted.
pub fn request_screen_capture_access() -> bool {
    CGRequestScreenCaptureAccess()
}

pub fn system_prefers_dark() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "Dark")
}

pub fn user_locale() -> Option<String> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

pub fn open_path(path: &str) -> Result<()> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| PlatformError::Os(e.to_string()))
}
