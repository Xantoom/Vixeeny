// SPDX-License-Identifier: GPL-3.0-or-later
#![windows_subsystem = "windows"]
//! The always-on Vixeeny daemon (plan section 1.2).

use vixeeny_common::config::Config;
use vixeeny_common::ipc::{BindError, ControlRequest, Endpoint};
use vixeeny_daemon::platform::{self, Startup};
use vixeeny_daemon::server::{AppLink, ask_running_daemon};

fn main() {
    vixeeny_common::logging::init("vixeeny-daemon");
    if let Err(e) = real_main() {
        tracing::error!("fatal: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> anyhow::Result<()> {
    // The frozen screens are placed in physical pixels.
    vixeeny_platform::ensure_dpi_aware();
    let endpoint = Endpoint::current_user();
    // Binding the endpoint is the single-instance lock.
    let listener = match endpoint.bind() {
        Ok(listener) => listener,
        Err(BindError::AlreadyRunning) => {
            tracing::info!("daemon already running, asking it to open the settings");
            ask_running_daemon(&endpoint, ControlRequest::OpenSettings)?;
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let config_path = vixeeny_common::paths::config_file();
    let config = match &config_path {
        Some(path) => Config::load(path).unwrap_or_else(|e| {
            tracing::warn!("using the default configuration: {e}");
            Config::default()
        }),
        None => Config::default(),
    };
    tracing::info!("daemon started (pid {})", std::process::id());
    vixeeny_daemon::supervisor::warm_probe();
    platform::run(Startup {
        config,
        config_path,
        listener,
        link: AppLink::default(),
    })
}
