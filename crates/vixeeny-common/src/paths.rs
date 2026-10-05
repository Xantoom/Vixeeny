// SPDX-License-Identifier: GPL-3.0-or-later
//! OS directories. `VIXEENY_CONFIG_DIR` / `VIXEENY_LOG_DIR` override them (tests, benchmarks,
//! portable installs).

use std::path::PathBuf;

use directories::{ProjectDirs, UserDirs};

fn dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "Vixeeny")
}

/// `<folder of the executable>/data` when a `portable.flag` file sits next to the executable
/// (the portable zip, plan 10.1): settings and logs then travel with the folder.
pub fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    portable_dir_of(exe.parent()?)
}

fn portable_dir_of(exe_dir: &std::path::Path) -> Option<PathBuf> {
    exe_dir
        .join("portable.flag")
        .exists()
        .then(|| exe_dir.join("data"))
}

/// Directory holding `config.toml`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VIXEENY_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = portable_dir() {
        return Some(dir);
    }
    dirs().map(|d| d.config_dir().to_path_buf())
}

/// `config.toml` path.
pub fn config_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// Cached result of the hardware encoder probe (plan 6.3).
pub fn hw_cache_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("hw_cache.toml"))
}

/// Directory for data that can be rebuilt (gallery thumbnails).
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = portable_dir() {
        return Some(dir.join("cache"));
    }
    // %LOCALAPPDATA%\Vixeeny\cache
    dirs().map(|d| d.cache_dir().to_path_buf())
}

/// Directory for rotating log files.
pub fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VIXEENY_LOG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = portable_dir() {
        return Some(dir.join("logs"));
    }
    // %LOCALAPPDATA%\Vixeeny\data\logs
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

/// A `[paths]` setting as people read it: placeholders expanded, Windows separators.
pub fn display_dir(template: &str) -> String {
    expand_user_dir(template)
        .map_or_else(|| template.to_owned(), |p| p.to_string_lossy().into_owned())
        .replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_shown_without_placeholders_or_forward_slashes() {
        let shown = display_dir("{videos}/Vixeeny/Replays");
        assert!(!shown.contains('{') && !shown.contains('/'), "{shown}");
        assert!(shown.ends_with("Vixeeny\\Replays"), "{shown}");
        assert_eq!(display_dir(r"D:\Captures"), r"D:\Captures");
    }

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

#[cfg(test)]
mod portable_tests {
    use super::*;

    #[test]
    fn the_flag_next_to_the_executable_makes_the_install_portable() {
        let dir = std::env::temp_dir().join(format!("vixeeny-portable-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(portable_dir_of(&dir), None);
        let _ = std::fs::write(dir.join("portable.flag"), "");
        assert_eq!(portable_dir_of(&dir), Some(dir.join("data")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
