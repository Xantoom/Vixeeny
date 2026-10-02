// SPDX-License-Identifier: GPL-3.0-or-later
//! What the last check found, kept in `update.json` next to the settings: the settings window
//! reads it to show the banner, the daemon to decide whether to notify.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::release::Release;

/// Appends a line to `update.log` next to the settings (the updater has no console).
pub fn log(message: &str) {
    use std::io::Write;
    let Some(dir) = vixeeny_common::paths::config_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("update.log"))
    {
        let _ = writeln!(file, "{message}");
    }
}

pub fn file() -> Option<PathBuf> {
    vixeeny_common::paths::config_dir().map(|d| d.join("update.json"))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Seconds since the Unix epoch of the last successful check.
    pub checked_at: u64,
    /// The newer release, when there is one.
    pub available: Option<Release>,
    /// The version the user was already told about.
    pub notified: Option<String>,
    /// Why the last attempt to install failed (cleared by a new attempt or a success).
    pub error: Option<String>,
}

impl State {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// The release to offer, if it is newer than `current`.
    pub fn newer_than(&self, current: &str) -> Option<&Release> {
        self.available
            .as_ref()
            .filter(|r| crate::release::is_newer(current, &r.version))
    }

    /// A check is due once a day.
    pub fn due(&self, now: u64) -> bool {
        now.saturating_sub(self.checked_at) >= 24 * 3600
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_round_trips_and_a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("update.json");
        assert_eq!(State::load(&path), State::default());
        let state = State {
            checked_at: 5,
            available: Some(Release {
                version: "1.0.0".into(),
                notes: "n".into(),
                assets: Vec::new(),
            }),
            notified: Some("1.0.0".into()),
            error: Some("boom".into()),
        };
        state.save(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(State::load(&path), state);
        std::fs::write(&path, "{broken").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(State::load(&path), State::default());
    }

    #[test]
    fn an_offer_older_than_the_running_version_is_dropped() {
        let release = Release {
            version: "0.5.0".into(),
            notes: String::new(),
            assets: Vec::new(),
        };
        let state = State {
            available: Some(release),
            ..State::default()
        };
        assert!(state.newer_than("0.4.0").is_some());
        assert!(state.newer_than("0.5.0").is_none());
        assert!(state.newer_than("0.6.0").is_none());
    }

    #[test]
    fn a_check_is_due_after_a_day() {
        let state = State {
            checked_at: 1000,
            ..State::default()
        };
        assert!(!state.due(1000 + 3600));
        assert!(state.due(1000 + 24 * 3600));
    }
}
