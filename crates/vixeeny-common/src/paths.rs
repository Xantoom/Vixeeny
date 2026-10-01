// SPDX-License-Identifier: GPL-3.0-or-later
//! OS directories. `VIXEENY_CONFIG_DIR` / `VIXEENY_LOG_DIR` override them (tests, benchmarks,
//! portable installs).

use std::path::PathBuf;

use directories::{ProjectDirs, UserDirs};

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

/// Expands the `{pictures}` and `{videos}` placeholders of the `[paths]` settings. Unknown
/// placeholders are left alone; a leading `~` is not special.
pub fn expand_user_dir(template: &str) -> Option<PathBuf> {
    let user = UserDirs::new()?;
    let pictures = user
        .picture_dir()
        .map_or_else(|| user.home_dir().join("Pictures"), PathBuf::from);
    let videos = user
        .video_dir()
        .map_or_else(|| user.home_dir().join("Videos"), PathBuf::from);
    let expanded = template
        .replace("{pictures}", &pictures.to_string_lossy())
        .replace("{videos}", &videos.to_string_lossy());
    Some(PathBuf::from(expanded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_expanded() {
        if let Some(dir) = expand_user_dir("{pictures}/Vixeeny") {
            assert!(dir.ends_with("Vixeeny"));
            assert!(!dir.to_string_lossy().contains('{'));
        }
        assert_eq!(
            expand_user_dir("/abs/{other}").unwrap_or_default(),
            PathBuf::from("/abs/{other}")
        );
    }
}
