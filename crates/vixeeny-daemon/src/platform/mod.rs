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

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::run;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::run;
