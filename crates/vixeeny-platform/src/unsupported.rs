// SPDX-License-Identifier: GPL-3.0-or-later
//! Placeholder until the macOS (M21) and Linux (M22) integrations.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use crate::{MonitorInfo, PlatformError, Result, WindowId, WindowInfo};

pub fn ensure_dpi_aware() {}

pub fn monitors() -> Result<Vec<MonitorInfo>> {
    Err(PlatformError::Unsupported)
}

/// Nothing to do: the process already has a terminal.
pub fn attach_console() {}

/// Nanoseconds on a monotonic clock.
pub fn monotonic_ns() -> i64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos() as i64
}

pub fn gpu_adapters() -> Result<Vec<crate::GpuInfo>> {
    Err(PlatformError::Unsupported)
}

pub fn cursor_position() -> Result<(i32, i32)> {
    Err(PlatformError::Unsupported)
}

pub fn foreground_window() -> Result<Option<WindowInfo>> {
    Err(PlatformError::Unsupported)
}

pub fn top_level_windows() -> Result<Vec<WindowInfo>> {
    Err(PlatformError::Unsupported)
}

pub fn window_info(_id: WindowId) -> Result<Option<WindowInfo>> {
    Err(PlatformError::Unsupported)
}

pub fn set_noactivate_tool_window(_id: WindowId) -> Result<()> {
    Err(PlatformError::Unsupported)
}

pub fn system_prefers_dark() -> bool {
    false
}

pub fn animations_enabled() -> bool {
    true
}

pub fn apply_acrylic(_id: WindowId) -> Result<()> {
    Err(PlatformError::Unsupported)
}

pub fn exclude_from_capture(_id: WindowId) -> Result<()> {
    Err(PlatformError::Unsupported)
}

pub fn hdr_info(_monitor: crate::MonitorId) -> Option<crate::HdrInfo> {
    None
}

pub fn exe_metadata(_path: &str) -> crate::ExeMetadata {
    crate::ExeMetadata::default()
}

pub struct InstanceGuard;

pub fn single_instance(_name: &str) -> Option<InstanceGuard> {
    Some(InstanceGuard)
}

pub fn focus_window_titled(_title: &str) -> bool {
    false
}

pub fn open_path(_path: &str) -> Result<()> {
    Err(PlatformError::Unsupported)
}

pub fn recycle(_path: &str) -> Result<()> {
    Err(PlatformError::Unsupported)
}

pub fn user_locale() -> Option<String> {
    None
}
