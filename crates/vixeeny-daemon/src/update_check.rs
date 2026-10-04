// SPDX-License-Identifier: GPL-3.0-or-later
//! The daily update check (plan 10.2). The network client lives in the app: the daemon only
//! starts `vixeeny-app --update background` once a day (a windowed program, so nothing shows)
//! and the app checks, downloads and installs, or tells the user, by the settings.

use std::path::PathBuf;
use std::time::Duration;

use vixeeny_common::config::Config;

/// First check this long after the start, so it never competes with it.
const FIRST_DELAY: Duration = Duration::from_secs(90);
const PERIOD: Duration = Duration::from_secs(24 * 3600);

fn app_next_to_daemon() -> Option<PathBuf> {
    let path = std::env::current_exe()
        .ok()?
        .with_file_name("vixeeny-app.exe");
    path.exists().then_some(path)
}

/// Starts the checking thread (nothing happens when the setting is off).
pub fn spawn(config_path: Option<PathBuf>) {
    let Some(app) = app_next_to_daemon() else {
        return;
    };
    let result = std::thread::Builder::new()
        .name("update-check".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            std::thread::sleep(FIRST_DELAY);
            loop {
                let enabled = config_path
                    .as_deref()
                    .and_then(|p| Config::load(p).ok())
                    .is_none_or(|c| c.general.check_updates);
                if enabled {
                    match std::process::Command::new(&app)
                        .args(["--update", "background"])
                        .spawn()
                    {
                        Ok(mut child) => {
                            let _ = child.wait();
                        }
                        Err(e) => tracing::warn!("cannot start the update check: {e}"),
                    }
                }
                std::thread::sleep(PERIOD);
            }
        });
    if let Err(e) = result {
        tracing::warn!("cannot start the update check: {e}");
    }
}
