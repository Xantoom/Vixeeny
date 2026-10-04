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

/// Versions before 0.9.2 named this program `vixeeny-daemon.exe`, and their updater restarts it by
/// that name: the copy that the 0.9.2 archive ships under the old name hands over to `Vixeeny.exe`.
fn hand_over_to_new_name() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let legacy = exe
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("vixeeny-daemon.exe"));
    let new = exe.with_file_name("Vixeeny.exe");
    legacy && new.exists() && std::process::Command::new(new).spawn().is_ok()
}

fn real_main() -> anyhow::Result<()> {
    if hand_over_to_new_name() {
        return Ok(());
    }
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
