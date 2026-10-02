// SPDX-License-Identifier: GPL-3.0-or-later
//! The daily update check (plan 10.2). The network client lives in `vixeeny-updater`, not here:
//! the daemon only starts it once a day and reads the one line it prints.

use std::path::{Path, PathBuf};
use std::time::Duration;

use vixeeny_common::config::Config;

use crate::core::Event;
use crate::server::EventTx;

/// First check this long after the start, so it never competes with it.
const FIRST_DELAY: Duration = Duration::from_secs(90);
const PERIOD: Duration = Duration::from_secs(24 * 3600);

/// `new <version>` from the updater's output.
pub fn parse_output(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|l| l.strip_prefix("new "))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn updater_next_to(exe: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "vixeeny-updater.exe"
    } else {
        "vixeeny-updater"
    };
    let path = exe.parent()?.join(name);
    path.exists().then_some(path)
}

fn check_once(updater: &Path) -> Option<String> {
    let output = std::process::Command::new(updater)
        .arg("check")
        .output()
        .ok()?;
    if !output.status.success() {
        tracing::debug!(
            "update check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    parse_output(&String::from_utf8_lossy(&output.stdout))
}

/// Starts the checking thread (nothing happens without the updater next to the daemon, as in a
/// package-manager install, or when the setting is off).
pub fn spawn(config_path: Option<PathBuf>, tx: EventTx) {
    let Some(updater) = std::env::current_exe()
        .ok()
        .and_then(|e| updater_next_to(&e))
    else {
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
                if enabled && let Some(version) = check_once(&updater) {
                    tx.send(Event::UpdateAvailable(version));
                }
                std::thread::sleep(PERIOD);
            }
        });
    if let Err(e) = result {
        tracing::warn!("cannot start the update check: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_new_version_line_counts() {
        assert_eq!(parse_output("new 1.2.0\n"), Some("1.2.0".into()));
        assert_eq!(parse_output("noise\nnew 0.5.1\r\n"), Some("0.5.1".into()));
        assert_eq!(parse_output(""), None);
        assert_eq!(parse_output("new \n"), None);
    }
}
