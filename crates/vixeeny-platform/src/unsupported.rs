// SPDX-License-Identifier: GPL-3.0-or-later
//! Placeholder until the macOS (M21) and Linux (M22) integrations.

use crate::{MonitorInfo, PlatformError, Result, WindowId, WindowInfo};

pub fn ensure_dpi_aware() {}

pub fn monitors() -> Result<Vec<MonitorInfo>> {
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

pub fn exclude_from_capture(_id: WindowId) -> Result<()> {
    Err(PlatformError::Unsupported)
}
