// SPDX-License-Identifier: GPL-3.0-or-later
//! What the last check found, kept in `update.json` next to the settings: the settings window and the background check
//! read it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::release::Release;

/// Appends a line to `update.log` next to the settings (the update runs without a window).
pub fn log(message: &str) {
    use std::io::Write;
    let Some(dir) = vixeeny_common::paths::config_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("update.log")) {
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
    /// The version that is downloaded, verified and waiting to be installed.
    #[serde(default)]
    pub ready: Option<String>,
    /// Why the last attempt to install failed (cleared by a new attempt or a success).
    pub error: Option<String>,
    /// The version whose install failed: not downloaded and installed again by itself.
    #[serde(default)]
    pub failed: Option<String>,
    /// The release of the version running, once known (for what changed in it).
    #[serde(default)]
    pub installed: Option<Release>,
    /// The version that ran last: another one now means Vixeeny was updated.
    #[serde(default)]
    pub last_run: Option<String>,
}

impl State {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // The check in the background and the settings window both write it: a reader never finds it half written.
        vixeeny_common::paths::write_atomic(path, text.as_bytes())
    }

    /// The release to offer, if it is newer than `current`.
    pub fn newer_than(&self, current: &str) -> Option<&Release> {
        self.available.as_ref().filter(|r| crate::release::is_newer(current, &r.version))
    }

    /// What changed in `version`, when its release is known.
    pub fn notes_of(&self, version: &str) -> Option<&str> {
        [&self.available, &self.installed]
            .into_iter()
            .flatten()
            .find(|r| r.version == version && !r.notes.is_empty())
            .map(|r| r.notes.as_str())
    }

    /// Records that `current` runs; `true` when it was updated to since the last run. With no run recorded (a version
    /// from before this record), the update is the one this file offered.
    pub fn arrived(&mut self, current: &str) -> bool {
        let updated = match &self.last_run {
            Some(last) => last != current,
            None => self.available.as_ref().is_some_and(|r| r.version == current),
        };
        self.last_run = Some(current.to_owned());
        updated
    }

    /// A check is due every 12 hours.
    pub fn due(&self, now: u64) -> bool {
        now.saturating_sub(self.checked_at) >= 12 * 3600
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_round_trips_and_a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.json");
        assert_eq!(State::load(&path), State::default());
        let state = State {
            checked_at: 5,
            available: Some(Release {
                version: "1.0.0".into(),
                assets: Vec::new(),
                signatures: [("a.zip".to_owned(), "sig".to_owned())].into(),
                notes: "- Things".into(),
            }),
            notified: Some("1.0.0".into()),
            ready: Some("1.0.0".into()),
            error: Some("boom".into()),
            failed: Some("0.9.0".into()),
            installed: None,
            last_run: Some("0.9.0".into()),
        };
        state.save(&path).unwrap();
        assert_eq!(State::load(&path), state);
        // Saved in place: nothing else is left in the folder.
        let files = std::fs::read_dir(dir.path()).map_or(0, Iterator::count);
        assert_eq!(files, 1);
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(State::load(&path), State::default());
    }

    #[test]
    fn an_offer_older_than_the_running_version_is_dropped() {
        let release = Release { version: "0.5.0".into(), assets: Vec::new(), ..Release::default() };
        let state = State { available: Some(release), ..State::default() };
        assert!(state.newer_than("0.4.0").is_some());
        assert!(state.newer_than("0.5.0").is_none());
        assert!(state.newer_than("0.6.0").is_none());
    }

    #[test]
    fn an_update_is_noticed_once_at_the_first_run_of_the_new_version() {
        let release = Release { version: "1.1.0".into(), notes: "- New".into(), ..Release::default() };
        // A first install: nothing to tell.
        let mut state = State::default();
        assert!(!state.arrived("1.0.0"));
        assert!(!state.arrived("1.0.0"));
        assert!(state.arrived("1.1.0"));
        assert!(!state.arrived("1.1.0"));
        // From a version that kept no record: the release this file offered.
        let mut state = State { available: Some(release), ..State::default() };
        assert!(state.arrived("1.1.0"));
        assert_eq!(state.notes_of("1.1.0"), Some("- New"));
        assert_eq!(state.notes_of("1.0.0"), None);
    }

    #[test]
    fn a_check_is_due_after_a_day() {
        let state = State { checked_at: 1000, ..State::default() };
        assert!(!state.due(1000 + 3600));
        assert!(state.due(1000 + 24 * 3600));
    }
}
