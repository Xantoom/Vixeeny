// SPDX-License-Identifier: GPL-3.0-or-later
//! OS directories. `VIXEENY_CONFIG_DIR` / `VIXEENY_LOG_DIR` override them (tests, benchmarks,
//! portable installs).

use std::path::PathBuf;

use directories::ProjectDirs;

fn dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "Vixeeny")
}

/// Directory holding `config.toml`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VIXEENY_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs().map(|d| d.config_dir().to_path_buf())
}

/// `config.toml` path.
pub fn config_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// Directory for rotating log files.
pub fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VIXEENY_LOG_DIR") {
        return Some(PathBuf::from(dir));
    }
    // Windows: %LOCALAPPDATA%\Vixeeny\data\logs ; Linux: ~/.local/state or share ; macOS: Logs.
    dirs().map(|d| {
        d.state_dir()
            .unwrap_or_else(|| d.data_local_dir())
            .join("logs")
    })
}
