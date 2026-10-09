// SPDX-License-Identifier: GPL-3.0-or-later
//! `config.toml`: typed model with defaults (plan annex 13.1), versioned schema and migrations.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ipc::ActionId;

/// Schema version written by this build. Bump it and add a migration to [`MIGRATIONS`].
pub const SCHEMA_VERSION: u32 = 4;

/// `MIGRATIONS[n]` upgrades a table from schema `n + 1` to `n + 2`.
pub type Migration = fn(&mut toml::Table);
pub const MIGRATIONS: &[Migration] = &[v1_fragmented_mp4, v2_single_profile, v3_naming_per_kind];

/// 0.9 → 1.0: MP4 recordings are fragmented (a recording cut short still plays), and the cursor
/// is left out of videos unless asked for again.
fn v1_fragmented_mp4(table: &mut toml::Table) {
    let Some(profiles) = table.get_mut("profiles").and_then(|p| p.as_table_mut()) else {
        return;
    };
    for (_, profile) in profiles.iter_mut() {
        let Some(profile) = profile.as_table_mut() else {
            continue;
        };
        if matches!(
            profile.get("container").and_then(|c| c.as_str()),
            Some("mp4" | "mp4_hybrid")
        ) {
            profile.insert("container".into(), "mp4_fragmented".into());
        }
        profile.insert("show_cursor".into(), false.into());
    }
}

/// 0.9.12: no more profiles. The one in use (`video.profile`, else `default`) becomes `[video]`.
fn v2_single_profile(table: &mut toml::Table) {
    let mut profiles = match table.remove("profiles") {
        Some(toml::Value::Table(profiles)) => profiles,
        _ => toml::Table::new(),
    };
    let name = table
        .get("video")
        .and_then(|v| v.get("profile"))
        .and_then(|p| p.as_str())
        .unwrap_or("default")
        .to_owned();
    match profiles
        .remove(&name)
        .or_else(|| profiles.remove("default"))
    {
        Some(profile) => table.insert("video".into(), profile),
        None => table.remove("video"),
    };
    if let Some(replay) = table.get_mut("replay").and_then(|r| r.as_table_mut()) {
        replay.remove("profile");
    }
}

/// 0.9.12: images, videos and replays are named each their own way. The one template becomes
/// that of the three (the "name after the game" switch is gone: `{app}` always is the game).
fn v3_naming_per_kind(table: &mut toml::Table) {
    let Some(paths) = table.get_mut("paths").and_then(|p| p.as_table_mut()) else {
        return;
    };
    let template = paths.remove("filename_template");
    paths.remove("use_foreground_app");
    let Some(template) = template else {
        return;
    };
    let mut one = toml::Table::new();
    one.insert("template".into(), template);
    let naming: toml::Table = ["images", "videos", "replays"]
        .into_iter()
        .map(|kind| (kind.to_owned(), toml::Value::Table(one.clone())))
        .collect();
    paths.insert("naming".into(), naming.into());
}

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
    pub recording_widget: RecordingWidget,
    pub replay: Replay,
    pub overlay: Overlay,
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
            recording_widget: RecordingWidget::default(),
            replay: Replay::default(),
            overlay: Overlay::default(),
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
    pub check_updates: bool,
    /// New versions are downloaded and installed in the background, without asking.
    pub auto_update: bool,
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
            check_updates: true,
            auto_update: true,
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
    pub record_toggle: Vec<String>,
    pub record_pause: Vec<String>,
    pub replay_toggle: Vec<String>,
    pub replay_save: Vec<String>,
    pub overlay_toggle: Vec<String>,
    pub open_settings: Vec<String>,
}

impl Hotkeys {
    /// Every action with the shortcuts configured for it, in [`ActionId::ALL`] order.
    pub fn bindings(&self) -> [(ActionId, &[String]); 11] {
        [
            (ActionId::CaptureRegion, &self.capture_region),
            (ActionId::CaptureWindow, &self.capture_window),
            (ActionId::CaptureFullscreen, &self.capture_fullscreen),
            (ActionId::CaptureAllMonitors, &self.capture_all_monitors),
            (ActionId::CaptureScrolling, &self.capture_scrolling),
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
    pub naming: PerKindNaming,
    pub per_app_subfolder: PerAppSubfolder,
    /// User table "executable file name → displayed name", consulted first (plan 5.8).
    pub app_names: BTreeMap<String, String>,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            images: "{pictures}/Vixeeny".into(),
            videos: "{videos}/Vixeeny".into(),
            replays: "{videos}/Vixeeny/Replays".into(),
            naming: PerKindNaming::default(),
            per_app_subfolder: PerAppSubfolder::default(),
            app_names: BTreeMap::new(),
        }
    }
}

/// How the files of one kind are named (plan 5.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Naming {
    /// `{app}`, `{date}`, `{time}`... are replaced. `{app}` of a capture that is not of a
    /// single window is the full-screen application (a game), else `Desktop`.
    pub template: String,
}

impl Default for Naming {
    fn default() -> Self {
        Self {
            template: "{app}_{date}_{time}".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerKindNaming {
    pub images: Naming,
    pub videos: Naming,
    pub replays: Naming,
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
    pub png: Png,
    pub jpeg: Jpeg,
    pub webp: Webp,
    pub avif: Avif,
    pub jxl: Jxl,
}

impl Default for Image {
    fn default() -> Self {
        Self {
            format: "png".into(),
            show_cursor: false,
            hdr: "tonemap_sdr".into(),
            copy_to_clipboard: false,
            png: Png::default(),
            jpeg: Jpeg::default(),
            webp: Webp::default(),
            avif: Avif::default(),
            jxl: Jxl::default(),
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
pub struct Png {
    /// `fast`, `default` or `high`.
    pub compression: String,
    /// The oxipng pass after it: 0 = none, else its level (1 fast to 6 slow).
    pub optimize: u8,
}

impl Default for Png {
    fn default() -> Self {
        Self {
            compression: "fast".into(),
            optimize: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Jpeg {
    pub quality: u8,
}

impl Default for Jpeg {
    fn default() -> Self {
        Self { quality: 90 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Webp {
    pub lossless: bool,
    /// 0 to 100, for the lossy mode.
    pub quality: u8,
    /// 0 (fast) to 6 (small).
    pub effort: u8,
}

impl Default for Webp {
    fn default() -> Self {
        Self {
            lossless: true,
            quality: 90,
            effort: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Jxl {
    pub lossless: bool,
    /// 1 to 100, for the lossy mode (90 is visually lossless).
    pub quality: u8,
    /// 1 (fast) to 9 (small).
    pub effort: u8,
}

impl Default for Jxl {
    fn default() -> Self {
        Self {
            lossless: true,
            quality: 90,
            effort: 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Avif {
    pub quality: u8,
    pub depth: u8,
    /// 0 (slow, small) to 10 (fast).
    pub speed: u8,
}

impl Default for Avif {
    fn default() -> Self {
        Self {
            quality: 80,
            depth: 10,
            speed: 6,
        }
    }
}

/// How videos (recordings and the replay) are encoded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    /// `hardware` or `software`: which family of encoders the settings list.
    pub encoder_kind: String,
    /// An encoder id of the codec registry. `auto` (older files) = the best detected hardware
    /// encoder, else libx264.
    pub encoder: String,
    pub container: String,
    pub resolution: String,
    /// `source` (the screen's) or `W:H`: the middle of the screen with that ratio.
    pub aspect: String,
    pub fps: u32,
    pub chroma: String,
    pub hdr: String,
    pub mode: String,
    /// `quality` (best picture), `small` (light files) or `custom` (`params`).
    pub preset: String,
    /// The encoder options of the `custom` preset, by the registry's parameter key.
    pub params: BTreeMap<String, String>,
    pub show_cursor: bool,
    /// Variable frame rate (Matroska and WebM only).
    pub vfr: bool,
    pub split: Split,
    pub audio: Audio,
}

impl Default for Video {
    fn default() -> Self {
        Self {
            encoder_kind: "hardware".into(),
            encoder: "auto".into(),
            container: "mp4_fragmented".into(),
            resolution: "source".into(),
            aspect: "source".into(),
            fps: 60,
            chroma: "420".into(),
            hdr: "tonemap_sdr".into(),
            mode: "simple".into(),
            preset: "quality".into(),
            params: BTreeMap::new(),
            show_cursor: false,
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
    /// What is recorded besides the microphone: `system`, `target` (the program in the
    /// foreground when the recording starts), `output`, `programs` or `none`. `sources` holds
    /// the matching sources.
    pub capture: String,
    pub sources: Vec<String>,
    /// `auto` = Opus beside AV1 (and VP9, and in WebM), else AAC.
    pub codec: String,
    pub bitrate_kbps: u32,
    pub vbr: bool,
    /// `mono`, `stereo`, `5.1` or `7.1`. Surround keeps 5.1/7.1 sources so (Matroska with
    /// Opus, FLAC or PCM; stereo elsewhere).
    pub channels: String,
    /// Remove steady background noise from microphone sources (FFmpeg `afftdn`).
    pub mic_noise_reduction: bool,
    /// Volume per source (`"mic" = 0.8`); missing = 1.0.
    pub volumes: BTreeMap<String, f32>,
    /// The tracks of `routing = "advanced"`.
    pub tracks: Vec<AudioTrack>,
}

/// The audio source that stands for the program in the foreground when the recording starts.
pub const TARGET_SOURCE: &str = "app:@target";

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
            capture: "system".into(),
            sources: vec!["system".into()],
            codec: "auto".into(),
            bitrate_kbps: 160,
            vbr: true,
            channels: "stereo".into(),
            mic_noise_reduction: false,
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
    /// The replay runs during every full-screen game: it starts with the game and stops when
    /// the game closes.
    #[serde(alias = "enabled_on_start")]
    pub enabled: bool,
    pub duration_seconds: u32,
    pub storage: String,
}

impl Default for Replay {
    fn default() -> Self {
        Self {
            enabled: false,
            duration_seconds: 30,
            storage: "auto".into(),
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
    fn older_files_record_fragmented_mp4_without_the_cursor() {
        let text = "schema_version = 1\n[profiles.default]\ncontainer = \"mp4_hybrid\"\n\
                    show_cursor = true\n[profiles.mkv]\ncontainer = \"mkv\"\n";
        let cfg = Config::from_toml(text).unwrap();
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
        assert_eq!(cfg.video.container, "mp4_fragmented");
        assert!(!cfg.video.show_cursor);
    }

    #[test]
    fn the_profile_in_use_becomes_the_video_settings() {
        let text = "schema_version = 2\n[video]\nprofile = \"Game\"\n\
                    [profiles.default]\nfps = 30\n[profiles.Game]\nfps = 120\n\
                    [replay]\nprofile = \"default\"\nduration_seconds = 60\n";
        let cfg = Config::from_toml(text).unwrap();
        assert_eq!(cfg.video.fps, 120);
        assert_eq!(cfg.replay.duration_seconds, 60);
        // A missing profile falls back to `default`, then to the defaults.
        let text =
            "schema_version = 2\n[video]\nprofile = \"Gone\"\n[profiles.default]\nfps = 30\n";
        assert_eq!(Config::from_toml(text).unwrap().video.fps, 30);
        let text = "schema_version = 2\n[video]\nprofile = \"Gone\"\n";
        assert_eq!(Config::from_toml(text).unwrap().video, Video::default());
        // The new layout reads back.
        let cfg = Config::default();
        let again = Config::from_toml(&cfg.to_toml().unwrap()).unwrap();
        assert_eq!(again, cfg);
    }

    #[test]
    fn one_naming_becomes_that_of_each_kind() {
        let text = "schema_version = 3\n[paths]\nfilename_template = \"{app}-{date}\"\n\
                    use_foreground_app = false\n";
        let cfg = Config::from_toml(text).unwrap();
        for naming in [
            &cfg.paths.naming.images,
            &cfg.paths.naming.videos,
            &cfg.paths.naming.replays,
        ] {
            assert_eq!(naming.template, "{app}-{date}");
        }
        // Without them, the defaults.
        let cfg = Config::from_toml("schema_version = 3\n[paths]\nimages = \"x\"\n").unwrap();
        assert_eq!(cfg.paths.naming, PerKindNaming::default());
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
