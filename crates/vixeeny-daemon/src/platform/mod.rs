// SPDX-License-Identifier: GPL-3.0-or-later
//! The OS-specific part: message loop, tray icon, wake-up of the loop from other threads.

use std::path::PathBuf;

use vixeeny_common::config::Config;
use vixeeny_common::ipc::Listener;

use crate::server::AppLink;

/// Everything the platform loop needs to start serving.
pub struct Startup {
    pub config: Config,
    pub config_path: Option<PathBuf>,
    pub listener: Listener,
    pub link: AppLink,
}

#[cfg(any(windows, target_os = "macos"))]
mod desktop;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod hotkeys;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::run;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod portal_shortcuts;
#[cfg(target_os = "linux")]
pub use linux::run;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod stub;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub use stub::run;
