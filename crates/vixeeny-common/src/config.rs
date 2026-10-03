// SPDX-License-Identifier: GPL-3.0-or-later
//! `config.toml`: typed model with defaults (plan annex 13.1), versioned schema and migrations.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ipc::ActionId;

/// Schema version written by this build. Bump it and add a migration to [`MIGRATIONS`].
pub const SCHEMA_VERSION: u32 = 1;

/// `MIGRATIONS[n]` upgrades a table from schema `n + 1` to `n + 2`.
pub type Migration = fn(&mut toml::Table);
pub const MIGRATIONS: &[Migration] = &[];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read or write {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot serialise config: {0}")]
    Serialise(#[from] toml::ser::Error),
    #[error("config schema {found} is newer than this build supports ({supported})")]
    TooNew { found: u32, supported: u32 },
    #[error("invalid schema_version in config")]
    BadVersion,
    #[error("no config directory available on this system")]
    NoConfigDir,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub general: General,
    pub hotkeys: Hotkeys,
    pub paths: Paths,
    pub image: Image,
    pub editor: Editor,
    pub video: Video,
    pub profiles: BTreeMap<String, Profile>,
    pub recording_widget: RecordingWidget,
    pub replay: Replay,
    pub overlay: Overlay,
    pub ocr: Ocr,
    pub scrolling: Scrolling,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: General::default(),
            hotkeys: Hotkeys::default(),
            paths: Paths::default(),
            image: Image::default(),
            editor: Editor::default(),
            video: Video::default(),
            profiles: BTreeMap::from([("default".to_owned(), Profile::default())]),
            recording_widget: RecordingWidget::default(),
            replay: Replay::default(),
            overlay: Overlay::default(),
            ocr: Ocr::default(),
            scrolling: Scrolling::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// `auto` or a language tag (`fr`, `en`).
    pub language: String,
    pub theme: String,
    pub autostart: bool,
    /// Seconds of inactivity before the app exits; 0 = exit immediately.
    pub app_idle_exit_seconds: u32,
    pub sounds: bool,
    pub notifications: bool,
    /// `native` (Windows notification, click opens the file) or `card` (Vixeeny's own).
    pub notification_style: String,
    pub check_updates: bool,
    /// The welcome assistant has been through (or skipped).
    pub first_run_done: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: "system".into(),
            autostart: true,
            app_idle_exit_seconds: 30,
            sounds: true,
            notifications: true,
            notification_style: "native".into(),
            check_updates: true,
            first_run_done: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub capture_region: Vec<String>,
    pub capture_window: Vec<String>,
    pub capture_fullscreen: Vec<String>,
    pub capture_all_monitors: Vec<String>,
    pub capture_scrolling: Vec<String>,
    pub ocr_region: Vec<String>,
    pub record_toggle: Vec<String>,
    pub record_pause: Vec<String>,
    pub replay_toggle: Vec<String>,
    pub replay_save: Vec<String>,
    pub overlay_toggle: Vec<String>,
    pub open_settings: Vec<String>,
}

impl Hotkeys {
    /// Every action with the shortcuts configured for it, in [`ActionId::ALL`] order.
    pub fn bindings(&self) -> [(ActionId, &[String]); 12] {
        [
            (ActionId::CaptureRegion, &self.capture_region),
            (ActionId::CaptureWindow, &self.capture_window),
            (ActionId::CaptureFullscreen, &self.capture_fullscreen),
            (ActionId::CaptureAllMonitors, &self.capture_all_monitors),
            (ActionId::CaptureScrolling, &self.capture_scrolling),
            (ActionId::OcrRegion, &self.ocr_region),
            (ActionId::RecordToggle, &self.record_toggle),
            (ActionId::RecordPause, &self.record_pause),
            (ActionId::ReplayToggle, &self.replay_toggle),
            (ActionId::ReplaySave, &self.replay_save),
            (ActionId::OverlayToggle, &self.overlay_toggle),
            (ActionId::OpenSettings, &self.open_settings),
        ]
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        let one = |s: &str| vec![s.to_owned()];
        Self {
            capture_region: one("PrintScreen"),
            capture_window: one("Alt+PrintScreen"),
            capture_fullscreen: one("Shift+PrintScreen"),
            capture_all_monitors: Vec::new(),
            capture_scrolling: Vec::new(),
            ocr_region: Vec::new(),
            record_toggle: one("Ctrl+Shift+R"),
            record_pause: one("Ctrl+Shift+P"),
            replay_toggle: Vec::new(),
            replay_save: one("Ctrl+Shift+S"),
            overlay_toggle: one("Ctrl+Shift+O"),
            open_settings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Paths {
    pub images: String,
    pub videos: String,
    pub replays: String,
    pub filename_template: String,
    pub per_app_subfolder: PerAppSubfolder,
    /// For captures that are not of a single window, `{app}` is the application that has the
    /// focus (plan 5.8); otherwise it is `Vixeeny`.
    pub use_foreground_app: bool,
    /// User table "executable file name → displayed name", consulted first (plan 5.8).
    pub app_names: BTreeMap<String, String>,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            images: "{pictures}/Vixeeny".into(),
            videos: "{videos}/Vixeeny".into(),
            replays: "{videos}/Vixeeny/Replays".into(),
            filename_template: "{app}_{date}_{time}".into(),
            per_app_subfolder: PerAppSubfolder::default(),
            use_foreground_app: true,
            app_names: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerAppSubfolder {
    pub images: bool,
    pub videos: bool,
    pub replays: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Image {
    pub format: String,
    pub show_cursor: bool,
    pub hdr: String,
    pub copy_to_clipboard: bool,
    pub jpeg: Jpeg,
    pub avif: Avif,
}

impl Default for Image {
    fn default() -> Self {
        Self {
            format: "png".into(),
            show_cursor: false,
            hdr: "tonemap_sdr".into(),
            copy_to_clipboard: false,
            jpeg: Jpeg::default(),
            avif: Avif::default(),
        }
    }
}

/// The Print Screen editor (plan 5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    /// Opacity of the dark veil over the frozen screen outside the zone, in percent.
    pub dim_percent: u8,
}

impl Default for Editor {
    fn default() -> Self {
        Self { dim_percent: 40 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Jpeg {
    pub quality: u8,
    pub chroma: String,
}

impl Default for Jpeg {
    fn default() -> Self {
        Self {
            quality: 90,
            chroma: "444".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Avif {
    pub quality: u8,
    pub depth: u8,
    pub chroma: String,
}

impl Default for Avif {
    fn default() -> Self {
        Self {
            quality: 80,
            depth: 10,
            chroma: "444".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub profile: String,
}

impl Default for Video {
    fn default() -> Self {
        Self {
            profile: "default".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// `auto` = best detected hardware encoder, else libx264.
    pub encoder: String,
    pub container: String,
    pub resolution: String,
    pub fps: u32,
    pub depth: u8,
    pub chroma: String,
    pub hdr: String,
    pub mode: String,
    pub preset: String,
    pub show_cursor: bool,
    /// Variable frame rate (Matroska and WebM only).
    pub vfr: bool,
    pub split: Split,
    pub audio: Audio,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            encoder: "auto".into(),
            container: "mp4_hybrid".into(),
            resolution: "source".into(),
            fps: 60,
            depth: 8,
            chroma: "420".into(),
            hdr: "tonemap_sdr".into(),
            mode: "simple".into(),
            preset: "balanced".into(),
            show_cursor: true,
            vfr: false,
            split: Split::default(),
            audio: Audio::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Split {
    pub mode: String,
}

impl Default for Split {
    fn default() -> Self {
        Self { mode: "off".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub routing: String,
    pub sources: Vec<String>,
    /// `auto` = AAC in MP4, Opus in MKV/WebM.
    pub codec: String,
    pub bitrate_kbps: u32,
    pub vbr: bool,
    /// Volume per source (`"mic" = 0.8`); missing = 1.0.
    pub volumes: BTreeMap<String, f32>,
    /// The tracks of `routing = "advanced"`.
    pub tracks: Vec<AudioTrack>,
}

/// A track of the advanced routing: a name and the sources mixed into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AudioTrack {
    pub name: String,
    pub sources: Vec<String>,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            routing: "one_track_per_source".into(),
            sources: vec!["system".into()],
            codec: "auto".into(),
            bitrate_kbps: 160,
            vbr: true,
            volumes: BTreeMap::new(),
            tracks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingWidget {
    pub enabled: bool,
    pub corner: String,
    pub auto_hide: bool,
}

impl Default for RecordingWidget {
    fn default() -> Self {
        Self {
            enabled: true,
            corner: "top_left".into(),
            auto_hide: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Replay {
    pub enabled_on_start: bool,
    pub duration_seconds: u32,
    pub storage: String,
    /// The video profile of the replay; empty = the recording's.
    pub profile: String,
}

impl Default for Replay {
    fn default() -> Self {
        Self {
            enabled_on_start: false,
            duration_seconds: 30,
            storage: "ram".into(),
            profile: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    pub edge: String,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            edge: "right".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ocr {
    pub languages: Vec<String>,
}

impl Default for Ocr {
    fn default() -> Self {
        Self {
            languages: vec!["auto".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scrolling {
    /// The assembled image is cut at this height (plan 5.7).
    pub max_height: u32,
    /// Open the assembled image in the editor before saving it.
    pub annotate: bool,
}

impl Default for Scrolling {
    fn default() -> Self {
        Self {
            max_height: 30_000,
            annotate: true,
        }
    }
}

/// Upgrades `table` to `current` by applying `migrations` in order.
pub fn migrate_with(
    table: &mut toml::Table,
    current: u32,
    migrations: &[Migration],
) -> Result<(), ConfigError> {
    let found = match table.get("schema_version") {
        None => 1,
        Some(v) => v
            .as_integer()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n >= 1)
            .ok_or(ConfigError::BadVersion)?,
    };
    if found > current {
        return Err(ConfigError::TooNew {
            found,
            supported: current,
        });
    }
    for version in found..current {
        let step = migrations
            .get(version as usize - 1)
            .ok_or(ConfigError::BadVersion)?;
        step(table);
        table.insert(
            "schema_version".into(),
            toml::Value::Integer(i64::from(version) + 1),
        );
    }
    Ok(())
}

impl Config {
    /// Parses and migrates a config document. Unknown keys are ignored, missing keys default.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let mut table: toml::Table = toml::from_str(text)?;
        migrate_with(&mut table, SCHEMA_VERSION, MIGRATIONS)?;
        Ok(table.try_into()?)
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Loads `path`; a missing file yields the defaults (nothing is written).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    /// Loads the config from the OS config directory.
    pub fn load_default_location() -> Result<Self, ConfigError> {
        Self::load(&crate::paths::config_file().ok_or(ConfigError::NoConfigDir)?)
    }

    /// Atomically writes the config (temporary file + rename).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |source| ConfigError::Io {
            path: path.display().to_string(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml()?).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips() {
        let cfg = Config::default();
        assert_eq!(Config::from_toml(&cfg.to_toml().unwrap()).unwrap(), cfg);
    }

    #[test]
    fn default_snapshot() {
        insta::assert_snapshot!(Config::default().to_toml().unwrap());
    }

    #[test]
    fn empty_document_is_default() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn partial_document_keeps_other_defaults() {
        let cfg = Config::from_toml("[general]\nautostart = false\n").unwrap();
        assert!(!cfg.general.autostart);
        assert_eq!(cfg.general.app_idle_exit_seconds, 30);
        assert_eq!(cfg.hotkeys, Hotkeys::default());
    }

    #[test]
    fn newer_schema_is_refused() {
        let err = Config::from_toml("schema_version = 99\n").unwrap_err();
        assert!(matches!(err, ConfigError::TooNew { found: 99, .. }));
    }

    #[test]
    fn bad_schema_version_is_refused() {
        for text in ["schema_version = 0\n", "schema_version = \"x\"\n"] {
            assert!(matches!(
                Config::from_toml(text),
                Err(ConfigError::BadVersion)
            ));
        }
    }

    #[test]
    fn migrations_run_in_order() {
        fn v1_to_v2(t: &mut toml::Table) {
            let old = t.remove("old_name").unwrap();
            t.insert("new_name".into(), old);
        }
        fn v2_to_v3(t: &mut toml::Table) {
            t.insert("added".into(), toml::Value::Boolean(true));
        }
        let mut table: toml::Table = toml::from_str("schema_version = 1\nold_name = 7\n").unwrap();
        migrate_with(&mut table, 3, &[v1_to_v2, v2_to_v3]).unwrap();
        assert_eq!(table["schema_version"].as_integer(), Some(3));
        assert_eq!(table["new_name"].as_integer(), Some(7));
        assert_eq!(table["added"].as_bool(), Some(true));
    }

    #[test]
    fn save_then_load() {
        let dir = std::env::temp_dir().join(format!("vx-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.general.language = "fr".into();
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = std::env::temp_dir().join("vx-definitely-missing/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }
}
