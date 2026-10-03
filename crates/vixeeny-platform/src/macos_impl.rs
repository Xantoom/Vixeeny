// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS: displays, cursor and the screen-recording permission through CoreGraphics. Positions
//! are physical pixels like on Windows: a display's bounds (points) times its backing scale.
//!
//! What is not here yet falls back to [`crate::unsupported`] (windows list, HDR, …).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayPixelsHigh, CGDisplayPixelsWide, CGEvent, CGGetActiveDisplayList,
    CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};

pub use crate::unsupported::*;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use crate::{MonitorId, MonitorInfo, PhysicalRect, PlatformError, Result, WindowId, WindowInfo};

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
    let scale = scale_at(at.x, at.y)?;
    Ok(((at.x * scale).round() as i32, (at.y * scale).round() as i32))
}

/// Backing scale of the display holding the point (in points), 1.0 when none does.
fn scale_at(x: f64, y: f64) -> Result<f64> {
    let primary = CGMainDisplayID();
    Ok(display_ids()?
        .into_iter()
        .map(|id| (describe(id, primary), CGDisplayBounds(id)))
        .find(|(_, b)| {
            x >= b.origin.x
                && x < b.origin.x + b.size.width
                && y >= b.origin.y
                && y < b.origin.y + b.size.height
        })
        .map_or(1.0, |(m, _)| f64::from(m.dpi) / 96.0))
}

unsafe extern "C" {
    /// libproc: the executable path of a process.
    fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, size: u32) -> i32;
}

fn exe_path_of(pid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    // SAFETY: the buffer is valid for `size` bytes; the call returns how many it wrote.
    let n = unsafe { proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buffer[..n as usize]).into_owned())
}

type Dict = NSDictionary<NSString, AnyObject>;

fn number(dict: &Dict, key: &str) -> Option<f64> {
    let value = dict.objectForKey(&NSString::from_str(key))?;
    value.downcast_ref::<NSNumber>().map(NSNumber::as_f64)
}

/// The visible application windows, frontmost first (layer 0, on screen, not tiny).
pub fn top_level_windows() -> Result<Vec<WindowInfo>> {
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )
    .ok_or_else(|| PlatformError::Os("cannot list the windows".into()))?;
    // SAFETY: a CFArray is an NSArray (toll-free bridging); ownership of the +1 moves over.
    let windows: Retained<NSArray<Dict>> = unsafe {
        Retained::from_raw(
            objc2_core_foundation::CFRetained::into_raw(list)
                .as_ptr()
                .cast(),
        )
    }
    .ok_or_else(|| PlatformError::Os("cannot read the window list".into()))?;
    let mut out = Vec::new();
    for dict in windows.iter() {
        if number(&dict, "kCGWindowLayer") != Some(0.0) {
            continue;
        }
        let Some(bounds) = dict
            .objectForKey(&NSString::from_str("kCGWindowBounds"))
            .and_then(|b| b.downcast::<NSDictionary>().ok())
            // SAFETY: window dictionaries are keyed by strings.
            .map(|b| unsafe { Retained::cast_unchecked::<Dict>(b) })
        else {
            continue;
        };
        let (Some(x), Some(y), Some(w), Some(h)) = (
            number(&bounds, "X"),
            number(&bounds, "Y"),
            number(&bounds, "Width"),
            number(&bounds, "Height"),
        ) else {
            continue;
        };
        let scale = scale_at(x, y)?;
        let rect = PhysicalRect::new(
            (x * scale).round() as i32,
            (y * scale).round() as i32,
            (w * scale).round() as u32,
            (h * scale).round() as u32,
        );
        if rect.width < 50 || rect.height < 50 {
            continue;
        }
        let pid = number(&dict, "kCGWindowOwnerPID").map_or(0, |p| p as u32);
        let title = dict
            .objectForKey(&NSString::from_str("kCGWindowName"))
            .and_then(|t| t.downcast::<NSString>().ok())
            .map_or_else(String::new, |t| t.to_string());
        out.push(WindowInfo {
            id: WindowId(number(&dict, "kCGWindowNumber").map_or(0, |n| n as u64)),
            title,
            rect,
            pid,
            exe_path: exe_path_of(pid),
        });
    }
    Ok(out)
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

#[repr(C)]
struct MachTimebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebase) -> i32;
}

/// Nanoseconds on the host clock `mach_absolute_time`, which is also the clock ScreenCaptureKit
/// dates its frames and audio with: video and audio line up without conversion.
pub fn monotonic_ns() -> i64 {
    static BASE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    let (numer, denom) = *BASE.get_or_init(|| {
        let mut info = MachTimebase { numer: 1, denom: 1 };
        // SAFETY: `info` is a valid out-parameter.
        unsafe { mach_timebase_info(&raw mut info) };
        (info.numer.max(1), info.denom.max(1))
    });
    // SAFETY: no preconditions.
    let ticks = unsafe { mach_absolute_time() };
    (u128::from(ticks) * u128::from(numer) / u128::from(denom)) as i64
}

/// `SCStreamConfiguration.captureMicrophone` exists from macOS 15.
pub fn microphone_via_screen_capture_kit() -> bool {
    use objc2::runtime::{AnyClass, Sel};
    AnyClass::get(c"SCStreamConfiguration").is_some_and(|c| {
        c.instance_method(Sel::register(c"setCaptureMicrophone:"))
            .is_some()
    })
}

/// The topmost window that is not ours: what the user was looking at when the shortcut fired.
pub fn foreground_window() -> Result<Option<WindowInfo>> {
    let own = std::process::id();
    Ok(top_level_windows()?.into_iter().find(|w| w.pid != own))
}

/// The application's name for file names: the `.app` bundle's, else nothing (the caller falls
/// back to the executable name).
pub fn exe_metadata(path: &str) -> crate::ExeMetadata {
    let product_name = path.split_once(".app/").and_then(|(before, _)| {
        std::path::Path::new(before)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    });
    crate::ExeMetadata {
        product_name,
        file_description: None,
    }
}

/// The `NSView` behind a window id (the winit handle of the window's content view).
///
/// # Safety
/// `id` must be the `NSView` pointer of a live window, used from the main thread.
unsafe fn view_of<'a>(id: WindowId) -> Result<&'a objc2_app_kit::NSView> {
    let ptr = id.0 as *const objc2_app_kit::NSView;
    // SAFETY: the caller guarantees the pointer designates a live view.
    unsafe { ptr.as_ref() }.ok_or_else(|| PlatformError::Os("no native view".into()))
}

/// Vibrancy: a behind-window `NSVisualEffectView` under the content, and a clear window so it
/// shows. `id` is the content `NSView` that winit reports.
pub fn apply_acrylic(id: WindowId) -> Result<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSColor, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| PlatformError::Os("not on the main thread".into()))?;
    // SAFETY: `id` comes from the live window of the caller, on the main thread (checked).
    let view = unsafe { view_of(id)? };
    let window = view
        .window()
        .ok_or_else(|| PlatformError::Os("the view has no window yet".into()))?;
    let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), view.bounds());
    effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    view.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    Ok(())
}

/// A floating window that shows on every Space and above full-screen apps and stays when the app
/// is not active: the recording pill. (A plain `NSWindow` cannot be made non-activating after
/// creation; clicking the pill may therefore bring the widget process forward.)
pub fn set_noactivate_tool_window(id: WindowId) -> Result<()> {
    use objc2_app_kit::{NSFloatingWindowLevel, NSWindowCollectionBehavior};

    // SAFETY: as in `apply_acrylic`.
    let view = unsafe { view_of(id)? };
    let window = view
        .window()
        .ok_or_else(|| PlatformError::Os("the view has no window yet".into()))?;
    window.setLevel(NSFloatingWindowLevel);
    window.setHidesOnDeactivate(false);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    Ok(())
}

/// Keeps the window out of screenshots and recordings (`sharingType = none`).
pub fn exclude_from_capture(id: WindowId) -> Result<()> {
    use objc2_app_kit::NSWindowSharingType;

    // SAFETY: as in `apply_acrylic`; `setSharingType` is thread-safe enough for a window we own.
    let view = unsafe { view_of(id)? };
    let window = view
        .window()
        .ok_or_else(|| PlatformError::Os("the view has no window yet".into()))?;
    window.setSharingType(NSWindowSharingType::None);
    Ok(())
}
