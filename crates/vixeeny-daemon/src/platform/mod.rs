// SPDX-License-Identifier: GPL-3.0-or-later
//! The Windows part: message loop, tray icon, global shortcuts, wake-up of the loop from other
//! threads.

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

mod freezer;
mod hotkeys;
mod tray;
mod windows;
pub use windows::run;
