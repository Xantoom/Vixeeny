// SPDX-License-Identifier: GPL-3.0-or-later
#![windows_subsystem = "windows"]
//! The always-on Vixeeny daemon (plan section 1.2).

use vixeeny_common::config::Config;
use vixeeny_common::ipc::{BindError, ControlRequest, Endpoint};
use vixeeny_daemon::platform::{self, Startup};
use vixeeny_daemon::server::{AppLink, ask_running_daemon};

fn main() {
    vixeeny_common::logging::init(
        "vixeeny-daemon",
        "daemon",
        log_clock,
        &vixeeny_platform::os_version(),
    );
    if let Err(e) = real_main() {
        tracing::error!("fatal: {e:#}");
        std::process::exit(1);
    }
}

/// The local time for the log lines.
fn log_clock() -> vixeeny_common::logging::Stamp {
    let t = vixeeny_platform::local_time();
    vixeeny_common::logging::Stamp {
        year: t.year,
        month: t.month,
        day: t.day,
        hour: t.hour,
        minute: t.minute,
        second: t.second,
        millisecond: t.millisecond,
        offset_minutes: vixeeny_platform::utc_offset_minutes(),
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
    vixeeny_daemon::supervisor::warm_probe();
    platform::run(Startup {
        config,
        config_path,
        listener,
        link: AppLink::default(),
    })
}
