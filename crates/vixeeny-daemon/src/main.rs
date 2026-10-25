// SPDX-License-Identifier: GPL-3.0-or-later
#![windows_subsystem = "windows"]
//! The always-on Vixeeny daemon.

use vixeeny_common::config::Config;
use vixeeny_common::ipc::{BindError, ControlRequest, Endpoint};
use vixeeny_daemon::platform::{self, Startup};
use vixeeny_daemon::server::{AppLink, ask_running_daemon};

fn main() {
    vixeeny_platform::harden_dll_search();
    vixeeny_common::logging::init(
        "vixeeny-daemon",
        "daemon",
        vixeeny_platform::local_time,
        &vixeeny_platform::os_version(),
    );
    if let Some(dir) = vixeeny_common::paths::crash_dir() {
        vixeeny_platform::crash::catch(&dir, "vixeeny-daemon", "daemon");
        vixeeny_platform::crash::prune(&dir);
    }
    if let Err(e) = real_main() {
        tracing::error!("fatal: {e:#}");
        std::process::exit(1);
    }
}

/// An update from a version before 0.20 may have left that version's daemon running (it answers under another name, see
/// [`Endpoint::older_user`]): it is asked to stop, as an update does, so that this one gets the shortcuts. A recording
/// it makes is finished first; it is waited for half a minute at most.
fn stop_older_daemon() {
    let Some(older) = Endpoint::older_user() else {
        return;
    };
    let pid = match older.ask_to_quit() {
        Ok(Some(pid)) => pid,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!("the daemon of an older version did not stop: {e}");
            return;
        }
    };
    tracing::info!("stopping the daemon of an older version (process {pid})");
    let (ended, end) = std::sync::mpsc::channel();
    if vixeeny_platform::on_process_exit(pid, move || {
        let _ = ended.send(());
    }) && end.recv_timeout(std::time::Duration::from_secs(30)).is_err()
    {
        tracing::warn!("the daemon of an older version is still running");
    }
}

/// An install in Program Files whose user gave Vixeeny the administrator's rights, started without them (the Start
/// menu, the start of the session): its task starts it again with them, and this one ends. `true` when it did.
fn start_elevated(endpoint: &Endpoint) -> bool {
    use vixeeny_platform::elevation::{self, Rights};
    if elevation::rights() != Rights::Limited {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    if !exe.parent().is_some_and(elevation::protected_install) || !elevation::task_starts(&exe) {
        return false;
    }
    // Already running: this start opens its settings, as any second start does.
    if endpoint.connect().is_ok() {
        return false;
    }
    match elevation::run_task() {
        Ok(()) => {
            tracing::info!("started again with the administrator's rights");
            true
        }
        Err(e) => {
            tracing::warn!("{e}");
            false
        }
    }
}

fn real_main() -> anyhow::Result<()> {
    // The frozen screens are placed in physical pixels.
    vixeeny_platform::ensure_dpi_aware();
    let endpoint = Endpoint::current_user();
    if start_elevated(&endpoint) {
        return Ok(());
    }
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
    stop_older_daemon();
    if let Err(e) = vixeeny_platform::crash::restart_after_crash() {
        tracing::warn!("no restart after a crash: {e}");
    }

    let config_path = vixeeny_common::paths::config_file();
    let config = match &config_path {
        // The very first start: starting with Windows is what the installer was told (the portable version, which has
        // no installer, does not).
        Some(path) if !path.exists() => {
            let mut config = Config::default();
            config.general.autostart = vixeeny_daemon::autostart::registered();
            if let Err(e) = config.save(path) {
                tracing::warn!("cannot write the first settings: {e}");
            }
            config
        }
        Some(path) => Config::load(path).unwrap_or_else(|e| {
            tracing::warn!("using the default configuration: {e}");
            Config::default()
        }),
        None => Config::default(),
    };
    vixeeny_daemon::supervisor::warm_probe();
    platform::run(Startup { config, config_path, listener, link: AppLink::default() })
}
