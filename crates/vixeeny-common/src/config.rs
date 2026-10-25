// SPDX-License-Identifier: GPL-3.0-or-later
//! `config.toml`: typed model with defaults, versioned schema and migrations.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ipc::ActionId;

/// Schema version written by this build. Bump it and add a migration to [`MIGRATIONS`].
pub const SCHEMA_VERSION: u32 = 10;

/// `MIGRATIONS[n]` upgrades a table from schema `n + 1` to `n + 2`.
pub type Migration = fn(&mut toml::Table);
pub const MIGRATIONS: &[Migration] = &[
    v1_fragmented_mp4,
    v2_single_profile,
    v3_naming_per_kind,
    v4_widget_display,
    v5_two_shortcuts,
    v6_scrolling_shortcut,
    v7_free_shortcuts,
    v8_new_defaults,
    v9_settings_of_their_own,
];

/// 0.9 → 1.0: MP4 recordings are fragmented (a recording cut short still plays), and the cursor is left out of videos
/// unless asked for again.
fn v1_fragmented_mp4(table: &mut toml::Table) {
    let Some(profiles) = table.get_mut("profiles").and_then(|p| p.as_table_mut()) else {
        return;
    };
    for (_, profile) in profiles.iter_mut() {
        let Some(profile) = profile.as_table_mut() else {
            continue;
        };
        if matches!(profile.get("container").and_then(|c| c.as_str()), Some("mp4" | "mp4_hybrid")) {
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
    let name =
        table.get("video").and_then(|v| v.get("profile")).and_then(|p| p.as_str()).unwrap_or("default").to_owned();
    match profiles.remove(&name).or_else(|| profiles.remove("default")) {
        Some(profile) => table.insert("video".into(), profile),
        None => table.remove("video"),
    };
    if let Some(replay) = table.get_mut("replay").and_then(|r| r.as_table_mut()) {
        replay.remove("profile");
    }
}

/// 0.9.12: images, videos and replays are named each their own way. The one template becomes that of the three (the
/// "name after the game" switch is gone: `{app}` always is the game).
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

/// 1.0: the widget's "hide when idle" switch becomes the second of three ways to show it.
fn v4_widget_display(table: &mut toml::Table) {
    let Some(widget) = table.get_mut("recording_widget").and_then(|w| w.as_table_mut()) else {
        return;
    };
    if widget.remove("auto_hide").and_then(|v| v.as_bool()) == Some(true) {
        widget.insert("display".into(), "fade".into());
    }
}

/// 1.0: two shortcuts per action, no longer three (the third is dropped).
fn v5_two_shortcuts(table: &mut toml::Table) {
    let Some(hotkeys) = table.get_mut("hotkeys").and_then(|h| h.as_table_mut()) else {
        return;
    };
    for list in hotkeys.iter_mut().filter_map(|(_, v)| v.as_array_mut()) {
        list.truncate(crate::hotkey::MAX_PER_ACTION);
    }
}

/// The scrolling capture's own shortcut.
const SCROLLING_SHORTCUT: &str = "Ctrl+PrintScreen";

/// 1.0: the scrolling capture has a shortcut of its own, unless the user has one for it or gave that one to another
/// action.
fn v6_scrolling_shortcut(table: &mut toml::Table) {
    let Some(hotkeys) = table.get_mut("hotkeys").and_then(|h| h.as_table_mut()) else {
        return;
    };
    let taken = hotkeys.values().filter_map(|v| v.as_array()).flatten().any(|v| {
        v.as_str().is_some_and(|text| {
            crate::hotkey::Hotkey::parse(text).ok() == crate::hotkey::Hotkey::parse(SCROLLING_SHORTCUT).ok()
        })
    });
    let empty = hotkeys.get("capture_scrolling").and_then(|v| v.as_array()).is_none_or(Vec::is_empty);
    if empty && !taken {
        hotkeys.insert("capture_scrolling".into(), toml::Value::Array(vec![SCROLLING_SHORTCUT.into()]));
    }
}

/// The shortcuts that were the defaults before 1.0 and the ones that replace them: Ctrl+Shift with a letter is taken by
/// browsers and editors (reload, command palette, save as).
const MOVED_SHORTCUTS: [(&str, &str, &str); 5] = [
    ("record_toggle", "Ctrl+Shift+R", "Ctrl+Shift+F9"),
    ("record_pause", "Ctrl+Shift+P", "Ctrl+Shift+F10"),
    ("replay_save", "Ctrl+Shift+S", "Ctrl+Shift+F11"),
    ("replay_edit", "Ctrl+Shift+E", "Ctrl+Shift+F12"),
    ("overlay_toggle", "Ctrl+Shift+O", "Ctrl+Shift+F8"),
];

/// 1.0: an action still on its former default shortcut moves to the new one, unless the user gave that one to an
/// action.
fn v7_free_shortcuts(table: &mut toml::Table) {
    if let Some(hotkeys) = table.get_mut("hotkeys").and_then(|h| h.as_table_mut()) {
        move_shortcuts(hotkeys, &MOVED_SHORTCUTS);
    }
}

/// The shortcuts that were the defaults before 1.0 and the ones that replace them: the screen and the scrolling capture
/// swap theirs, and the other ones drop Shift.
const MOVED_SHORTCUTS_V8: [(&str, &str, &str); 7] = [
    ("capture_fullscreen", "Shift+PrintScreen", "Ctrl+PrintScreen"),
    ("capture_scrolling", "Ctrl+PrintScreen", "Shift+PrintScreen"),
    ("record_toggle", "Ctrl+Shift+F9", "Ctrl+F9"),
    ("record_pause", "Ctrl+Shift+F10", "Ctrl+F10"),
    ("replay_save", "Ctrl+Shift+F11", "Ctrl+F11"),
    ("replay_edit", "Ctrl+Shift+F12", "Ctrl+F12"),
    ("overlay_toggle", "Ctrl+Shift+F8", "Ctrl+F8"),
];

/// The folders that were the defaults before 1.0 and the ones that replace them.
const MOVED_FOLDERS: [(&str, &str, &str); 3] = [
    ("images", "{pictures}/Vixeeny", crate::paths::DEFAULT_IMAGES),
    ("videos", "{videos}/Vixeeny", crate::paths::DEFAULT_VIDEOS),
    ("replays", "{videos}/Vixeeny/Replays", crate::paths::DEFAULT_REPLAYS),
];

/// 1.0: what is still on a former default moves to the new one. The shortcuts (unless the new one is held by an action
/// that keeps it), the folders, and MP4: hybrid rather than fragmented, as some editors read fragmented files badly.
fn v8_new_defaults(table: &mut toml::Table) {
    if let Some(hotkeys) = table.get_mut("hotkeys").and_then(|h| h.as_table_mut()) {
        move_shortcuts(hotkeys, &MOVED_SHORTCUTS_V8);
    }
    if let Some(paths) = table.get_mut("paths").and_then(|p| p.as_table_mut()) {
        for (kind, old, new) in MOVED_FOLDERS {
            if paths.get(kind).and_then(|v| v.as_str()) == Some(old) {
                paths.insert(kind.into(), new.into());
            }
        }
    }
    if let Some(video) = table.get_mut("video").and_then(|v| v.as_table_mut())
        && matches!(video.get("container").and_then(|c| c.as_str()), Some("mp4_fragmented" | "fmp4"))
    {
        video.insert("container".into(), "mp4_hybrid".into());
    }
}

/// The file name template that was the default before 1.0.
const OLD_TEMPLATE: &str = "{app}_{date}_{time}";

/// The shortcuts given to the actions that had none.
const NEW_SHORTCUTS: [(&str, &str); 2] = [("capture_all_monitors", "Ctrl+F7"), ("replay_toggle", "Ctrl+Shift+F11")];

/// 1.0: the notifications get a table of their own, the disk alert one per kind of video, the replay its own video and
/// sound settings (those of the videos so far) and switches for what a number turned off; the compact profile is called
/// balanced. Files still on the former default name join the new one, and the actions without a shortcut get theirs
/// when it is free.
fn v9_settings_of_their_own(table: &mut toml::Table) {
    let general = table.get_mut("general").and_then(|g| g.as_table_mut());
    let notifications = general.and_then(|g| g.remove("notifications")).and_then(|v| v.as_bool()).unwrap_or(true);
    // The compact profile is the balanced one now.
    if let Some(video) = table.get_mut("video").and_then(|v| v.as_table_mut())
        && video.get("preset").and_then(toml::Value::as_str) == Some("small")
    {
        video.insert("preset".into(), "balanced".into());
    }
    let replay_video = table.get("video").cloned();
    let mut replay = match table.remove("replay") {
        Some(toml::Value::Table(replay)) => replay,
        _ => toml::Table::new(),
    };
    let game_start = replay.remove("notify").and_then(|v| v.as_str().map(str::to_owned));
    // Before, the general switch left the game cards alone: they stay as they were.
    let mut cards = toml::Table::new();
    if let Some(game_start) = &game_start {
        cards.insert("game_start".into(), game_start.as_str().into());
    }
    if !notifications {
        if game_start.as_deref() == Some("none") {
            cards.insert("enabled".into(), false.into());
        } else {
            for kind in ["image_saved", "text_copied", "recording_saved", "replay_saved", "failures", "updates"] {
                cards.insert(kind.into(), false.into());
            }
        }
    }
    table.insert("notifications".into(), cards.into());
    // The minutes of a duration of whole minutes, else the seconds (up to a minute).
    if let Some(seconds) = replay.get("duration_seconds").and_then(toml::Value::as_integer).filter(|s| *s > 0) {
        let (amount, unit) = if seconds > 60 { ((seconds + 30) / 60, "minutes") } else { (seconds, "seconds") };
        replay.insert("duration_seconds".into(), (amount.clamp(1, 60) * if unit == "minutes" { 60 } else { 1 }).into());
        replay.insert("duration_unit".into(), unit.into());
    }
    if let Some(after) = replay.get("after_seconds").and_then(toml::Value::as_integer).filter(|s| *s >= 0) {
        replay.insert("after_enabled".into(), (after > 0).into());
        replay.insert("after_seconds".into(), if after > 0 { after.min(60) } else { 5 }.into());
    }
    if let Some(mut video) = replay_video.and_then(|v| match v {
        toml::Value::Table(video) => Some(video),
        _ => None,
    }) {
        // The cut into parts is the videos' alone.
        video.remove("split");
        replay.insert("video".into(), video.into());
    }
    table.insert("replay".into(), replay.into());
    let percent = table
        .get_mut("recording_widget")
        .and_then(|w| w.as_table_mut())
        .and_then(|w| w.remove("disk_alert_percent"))
        .and_then(|v| v.as_integer());
    if let Some(percent) = percent {
        let mut alert = toml::Table::new();
        alert.insert("enabled".into(), (percent > 0).into());
        alert.insert("percent".into(), if percent > 0 { percent.min(99) } else { 5 }.into());
        let mut alerts = toml::Table::new();
        alerts.insert("recordings".into(), alert.clone().into());
        alerts.insert("replays".into(), alert.into());
        table.insert("disk_alert".into(), alerts.into());
    }
    if let Some(naming) = table.get_mut("paths").and_then(|p| p.get_mut("naming")).and_then(|n| n.as_table_mut()) {
        for (_, kind) in naming.iter_mut() {
            if let Some(kind) = kind.as_table_mut()
                && kind.get("template").and_then(|t| t.as_str()) == Some(OLD_TEMPLATE)
            {
                kind.insert("template".into(), Naming::default().template.into());
            }
        }
    }
    if let Some(hotkeys) = table.get_mut("hotkeys").and_then(|h| h.as_table_mut()) {
        for (action, shortcut) in NEW_SHORTCUTS {
            let taken = hotkeys.values().filter_map(|v| v.as_array()).flatten().any(|v| {
                v.as_str().is_some_and(|text| {
                    crate::hotkey::Hotkey::parse(text).ok() == crate::hotkey::Hotkey::parse(shortcut).ok()
                })
            });
            let empty = hotkeys.get(action).and_then(|v| v.as_array()).is_none_or(Vec::is_empty);
            if empty && !taken {
                hotkeys.insert(action.into(), toml::Value::Array(vec![shortcut.into()]));
            }
        }
    }
}

/// Moves each action still on `old` to `new`, all at once (two actions may swap theirs). A move whose `new` is held by
/// a shortcut that stays where it is does not happen.
fn move_shortcuts(hotkeys: &mut toml::Table, moves: &[(&str, &str, &str)]) {
    use crate::hotkey::Hotkey;
    let parse = |text: &str| Hotkey::parse(text).ok();
    let holds = |hotkeys: &toml::Table, action: &str, key: &Option<Hotkey>| {
        hotkeys
            .get(action)
            .and_then(|v| v.as_array())
            .is_some_and(|list| list.iter().any(|v| v.as_str().is_some_and(|t| parse(t) == *key)))
    };
    // The moves that can happen: dropped one by one until each lands on a free shortcut.
    let mut moving: Vec<&(&str, &str, &str)> =
        moves.iter().filter(|(action, old, _)| holds(hotkeys, action, &parse(old))).collect();
    loop {
        let staying = |hotkeys: &toml::Table, moving: &[&(&str, &str, &str)], key: &Option<Hotkey>| {
            hotkeys.iter().any(|(action, list)| {
                list.as_array().is_some_and(|list| {
                    list.iter().any(|v| {
                        v.as_str().is_some_and(|t| {
                            parse(t) == *key && !moving.iter().any(|(a, old, _)| a == action && parse(old) == *key)
                        })
                    })
                })
            })
        };
        let blocked = moving.iter().position(|(_, _, new)| staying(hotkeys, &moving, &parse(new)));
        match blocked {
            Some(i) => {
                moving.remove(i);
            }
            None => break,
        }
    }
    for (action, old, new) in moving {
        let Some(list) = hotkeys.get_mut(*action).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        for entry in list.iter_mut() {
            if entry.as_str().is_some_and(|text| parse(text) == parse(old)) {
                *entry = (*new).into();
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read or write {path}: {source}")]
    Io { path: String, source: std::io::Error },
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
    pub video: Video,
    pub recording_widget: RecordingWidget,
    pub replay: Replay,
    pub editor: Editor,
    pub overlay: Overlay,
    pub scrolling: Scrolling,
    pub notifications: Notifications,
    pub disk_alert: DiskAlerts,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: General::default(),
            hotkeys: Hotkeys::default(),
            paths: Paths::default(),
            image: Image::default(),
            video: Video::default(),
            recording_widget: RecordingWidget::default(),
            replay: Replay::default(),
            editor: Editor::default(),
            overlay: Overlay::default(),
            scrolling: Scrolling::default(),
            notifications: Notifications::default(),
            disk_alert: DiskAlerts::default(),
        }
    }
}

impl Config {
    /// What the replay records with, sound included: its own settings, or those of the videos. Always a hardware
    /// encoder (the replay runs beside a game: the processor is the game's): the hardware one the settings had, else
    /// the best one.
    pub fn replay_video(&self) -> Video {
        let mut video = if self.replay.same_as_video { self.video.clone() } else { self.replay.video.clone() };
        video.split = Split::default();
        if video.encoder_kind == "software" {
            video.encoder_kind = "hardware".into();
            video.encoder = video.encoder_by_kind.get("hardware").cloned().unwrap_or_else(|| "auto".into());
        }
        video
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
    pub check_updates: bool,
    /// New versions are downloaded and installed in the background, without asking.
    pub auto_update: bool,
    /// A window tells what changed at the first start after an update.
    pub whats_new: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: "system".into(),
            autostart: true,
            app_idle_exit_seconds: 30,
            check_updates: true,
            auto_update: true,
            whats_new: true,
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
    pub replay_edit: Vec<String>,
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
            (ActionId::RecordToggle, &self.record_toggle),
            (ActionId::RecordPause, &self.record_pause),
            (ActionId::ReplayToggle, &self.replay_toggle),
            (ActionId::ReplaySave, &self.replay_save),
            (ActionId::ReplayEdit, &self.replay_edit),
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
            capture_fullscreen: one("Ctrl+PrintScreen"),
            capture_all_monitors: one(NEW_SHORTCUTS[0].1),
            capture_scrolling: one("Shift+PrintScreen"),
            record_toggle: one("Ctrl+F9"),
            record_pause: one("Ctrl+F10"),
            // Three keys: turning the replay off by mistake would lose the moment to keep.
            replay_toggle: one(NEW_SHORTCUTS[1].1),
            replay_save: one("Ctrl+F11"),
            replay_edit: one("Ctrl+F12"),
            overlay_toggle: one("Ctrl+F8"),
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
    /// User table "executable file name → displayed name", consulted first.
    pub app_names: BTreeMap<String, String>,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            images: crate::paths::DEFAULT_IMAGES.into(),
            videos: crate::paths::DEFAULT_VIDEOS.into(),
            replays: crate::paths::DEFAULT_REPLAYS.into(),
            naming: PerKindNaming::default(),
            per_app_subfolder: PerAppSubfolder::default(),
            app_names: BTreeMap::new(),
        }
    }
}

/// How the files of one kind are named.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Naming {
    /// `{app}`, `{date}`, `{time}`... are replaced. `{app}` of a capture that is not of a single window is the
    /// full-screen application (a game), else `Desktop`.
    pub template: String,
}

impl Default for Naming {
    fn default() -> Self {
        // Spaces, as Windows names its own captures: readable, and kept by the places files are shared to.
        Self { template: "{app} {date} {time}".into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerKindNaming {
    pub images: Naming,
    pub videos: Naming,
    pub replays: Naming,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerAppSubfolder {
    pub images: bool,
    pub videos: bool,
    pub replays: bool,
}

impl Default for PerAppSubfolder {
    fn default() -> Self {
        Self { images: true, videos: true, replays: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Image {
    pub format: String,
    pub show_cursor: bool,
    pub hdr: String,
    pub copy_to_clipboard: bool,
    /// What a window capture is set on, with a margin and a shadow: `none`, `wallpaper` (the desktop's, blurred),
    /// `colour` or `gradient`. `blur` (from before 0.21) reads as `wallpaper`.
    pub window_backdrop: String,
    /// The `colour` one: `#rrggbb`, or `accent` (Windows'); one of [`BACKDROP_COLOURS`] by its name, from before, reads
    /// as its colour.
    pub backdrop_colour: String,
    /// The `gradient` one: `#rrggbb,#rrggbb`, from the top left to the bottom right; one of [`BACKDROP_GRADIENTS`] by
    /// its name, from before, reads as its colours.
    pub backdrop_gradient: String,
    pub png: Png,
    pub jpeg: Jpeg,
    pub webp: Webp,
    pub avif: Avif,
    pub jxl: Jxl,
}

impl Default for Image {
    fn default() -> Self {
        Self {
            format: "jpeg".into(),
            show_cursor: false,
            hdr: "tonemap_sdr".into(),
            copy_to_clipboard: true,
            window_backdrop: "none".into(),
            backdrop_colour: "accent".into(),
            backdrop_gradient: "dusk".into(),
            png: Png::default(),
            jpeg: Jpeg::default(),
            webp: Webp::default(),
            avif: Avif::default(),
            jxl: Jxl::default(),
        }
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
        Self { compression: "default".into(), optimize: 0 }
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
        Self { lossless: true, quality: 90, effort: 4 }
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
        Self { lossless: true, quality: 90, effort: 7 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Avif {
    pub quality: u8,
    pub depth: u8,
    /// 0 (slow, small) to [`AVIF_MAX_SPEED`] (fast).
    pub speed: u8,
}

/// The fastest AVIF speed: libaom's `cpu-used` 8, FFmpeg's last.
pub const AVIF_MAX_SPEED: u8 = 6;

impl Default for Avif {
    fn default() -> Self {
        Self { quality: 80, depth: 10, speed: 6 }
    }
}

/// How videos (recordings and the replay) are encoded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    /// `hardware` or `software`: which family of encoders the settings list.
    pub encoder_kind: String,
    /// An encoder id of the codec registry. `auto` (older files) = the best detected hardware encoder, else libx264.
    pub encoder: String,
    /// The encoder chosen for the other kind, by kind: back when that kind is chosen again.
    pub encoder_by_kind: BTreeMap<String, String>,
    /// The codec `auto` keeps to when an encoder of the kind has it (`h264`); empty: the best of all (AV1, then HEVC,
    /// then H.264).
    pub auto_codec: String,
    pub container: String,
    pub resolution: String,
    /// `source` (the screen's) or `W:H`: the middle of the screen with that ratio.
    pub aspect: String,
    pub fps: u32,
    pub chroma: String,
    pub hdr: String,
    pub mode: String,
    /// `quality` (for editing and keeping), `balanced`, `performance` (balanced without the tools that load the
    /// encoder: the replay's) or `custom` (`params`). `small`, from before 1.0, reads as `balanced`.
    pub preset: String,
    /// The encoder options of the `custom` preset, by the registry's parameter key: those of the encoder in use.
    pub params: BTreeMap<String, String>,
    /// Those of the other encoders, by encoder id: back when that encoder is chosen again.
    pub params_by_encoder: BTreeMap<String, BTreeMap<String, String>>,
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
            encoder_by_kind: BTreeMap::new(),
            auto_codec: String::new(),
            container: "mp4_hybrid".into(),
            resolution: "source".into(),
            aspect: "source".into(),
            fps: 60,
            chroma: "420".into(),
            hdr: "tonemap_sdr".into(),
            mode: "simple".into(),
            preset: "balanced".into(),
            params: BTreeMap::new(),
            params_by_encoder: BTreeMap::new(),
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
    /// `off`, `size:<MB>` or `duration:<minutes>`.
    pub mode: String,
    /// The amounts of each kind, kept while another kind (or none) is chosen.
    pub size_mb: u32,
    pub minutes: u32,
}

impl Default for Split {
    fn default() -> Self {
        Self { mode: "off".into(), size_mb: 2_048, minutes: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub routing: String,
    /// What is recorded besides the microphone: `system`, `target` (the program in the foreground when the recording
    /// starts), `output`, `programs` or `none`. `sources` holds the matching sources.
    pub capture: String,
    pub sources: Vec<String>,
    /// The sources of the other modes (an output device, the programs ticked), back when their mode is chosen again.
    pub set_aside: Vec<String>,
    /// `auto` = Opus beside AV1 (and VP9, and in WebM), else AAC.
    pub codec: String,
    pub bitrate_kbps: u32,
    pub vbr: bool,
    /// `mono`, `stereo` or `surround`: 5.1 and 7.1 sources are kept so, up to 7.1 (Matroska with Opus, FLAC or PCM;
    /// stereo elsewhere). `5.1` and `7.1`, from before, read as `surround`.
    pub channels: String,
    /// Remove steady background noise from microphone sources (FFmpeg `afftdn`).
    pub mic_noise_reduction: bool,
    /// Volume per source (`"mic" = 0.8`); missing = 1.0.
    pub volumes: BTreeMap<String, f32>,
    /// The tracks of `routing = "advanced"`.
    pub tracks: Vec<AudioTrack>,
}

impl Video {
    /// The replay's settings until changed: light enough to run through every game. The encoder the videos' `auto`
    /// picks; 1080p at 60 FPS, where the networks stop; the performance profile (no look-ahead, B-frames, extra pass or
    /// adaptive quantisation); AAC in one track, what a web player plays. The editor makes the copy for a network.
    pub fn replay() -> Self {
        Self {
            resolution: "1080p".into(),
            preset: "performance".into(),
            audio: Audio { routing: "mix_all".into(), codec: "aac".into(), ..Audio::default() },
            ..Self::default()
        }
    }

    /// The encoder in use goes from `from` to `to` (ids): its custom options are put aside, and those of `to` taken
    /// back (none when it never had any).
    pub fn swap_params(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        let theirs = self.params_by_encoder.remove(to).unwrap_or_default();
        let mine = std::mem::replace(&mut self.params, theirs);
        if !mine.is_empty() {
            self.params_by_encoder.insert(from.to_owned(), mine);
        }
    }
}

/// The plain colours a window capture can be set on, besides Windows' accent (sRGB).
pub const BACKDROP_COLOURS: &[(&str, [u8; 3])] = &[
    ("white", [0xf3, 0xf3, 0xf3]),
    ("grey", [0x8a, 0x8a, 0x8a]),
    ("black", [0x1c, 0x1c, 0x1c]),
    ("blue", [0x00, 0x63, 0xb1]),
    ("purple", [0x74, 0x4d, 0xa9]),
    ("green", [0x10, 0x7c, 0x10]),
    ("orange", [0xca, 0x50, 0x10]),
    ("pink", [0xe3, 0x00, 0x8c]),
];

/// The gradients a window capture can be set on: from the top left to the bottom right (sRGB).
pub const BACKDROP_GRADIENTS: &[(&str, [u8; 3], [u8; 3])] = &[
    ("dusk", [0x65, 0x4e, 0xa3], [0xea, 0xaf, 0xc8]),
    ("sunset", [0xff, 0x51, 0x2f], [0xf0, 0x98, 0x19]),
    ("ocean", [0x21, 0x93, 0xb0], [0x6d, 0xd5, 0xed]),
    ("forest", [0x13, 0x4e, 0x5e], [0x71, 0xb2, 0x80]),
    ("night", [0x0f, 0x20, 0x27], [0x2c, 0x53, 0x64]),
];

/// `#rrggbb` (or `rrggbb`, `#rgb`) as a colour.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let digits = text.trim().trim_start_matches('#');
    let digits: String = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 => digits.to_owned(),
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// A colour as `#rrggbb`.
pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

impl Image {
    /// The plain colour a window capture is set on; `None`: Windows' accent.
    pub fn backdrop_colour(&self) -> Option<[u8; 3]> {
        BACKDROP_COLOURS
            .iter()
            .find(|(id, _)| *id == self.backdrop_colour)
            .map(|(_, c)| *c)
            .or_else(|| parse_hex(&self.backdrop_colour))
    }

    /// The ends of the gradient a window capture is set on.
    pub fn backdrop_gradient(&self) -> ([u8; 3], [u8; 3]) {
        let preset =
            |id: &str| BACKDROP_GRADIENTS.iter().find(|(name, ..)| *name == id).map(|(_, from, to)| (*from, *to));
        preset(&self.backdrop_gradient)
            .or_else(|| {
                let (from, to) = self.backdrop_gradient.split_once(',')?;
                Some((parse_hex(from)?, parse_hex(to)?))
            })
            .unwrap_or((BACKDROP_GRADIENTS[0].1, BACKDROP_GRADIENTS[0].2))
    }
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
            capture: "target".into(),
            sources: vec![TARGET_SOURCE.into()],
            set_aside: Vec::new(),
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
    /// `top_left`, `top_center`, `top_right`, `bottom_left`, `bottom_center`, `bottom_right`, or `custom`: at
    /// [`RecordingWidget::custom`].
    pub corner: String,
    /// The custom place: how far across the free room of the screen the widget is, from its left and top edges (0 to
    /// 1). Set in the settings.
    pub custom: (f32, f32),
    /// `always`, `fade` (it fades after 3 s and comes back under the pointer) or `alerts` (it only shows while
    /// something goes wrong).
    pub display: String,
}

impl Default for RecordingWidget {
    fn default() -> Self {
        Self { enabled: true, corner: "top_left".into(), custom: (0.5, 0.0), display: "always".into() }
    }
}

/// What the editor saved a part with last: kept for the next one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    /// `original` (the clip as it is), `custom` (the three below), or a network the copy is made for
    /// (`vixeeny_encode::share`'s ids), which sets them.
    pub export: String,
    /// `h264`, `hevc` or `av1`.
    pub codec: String,
    /// The picture's height; 0: the clip's.
    pub height: u32,
    /// Frames per second; 0: the clip's.
    pub fps: u32,
}

impl Default for Editor {
    fn default() -> Self {
        Self { export: "original".into(), codec: "h264".into(), height: 0, fps: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Replay {
    /// The replay runs during every full-screen game: it starts with the game and stops when the game closes.
    #[serde(alias = "enabled_on_start")]
    pub enabled: bool,
    pub duration_seconds: u32,
    /// How the duration is set: `seconds` or `minutes` (1 to 60 of them).
    pub duration_unit: String,
    /// The replay goes on recording for [`Replay::after_seconds`] after its shortcut, the end of the moment included.
    pub after_enabled: bool,
    pub after_seconds: u32,
    pub storage: String,
    /// The replay records with the video settings rather than [`Replay::video`].
    pub same_as_video: bool,
    /// The replay's own video and sound settings.
    #[serde(default = "Video::replay")]
    pub video: Video,
    /// Programs that are games even without a sign of it (executable names, lower case).
    pub always: Vec<String>,
    /// Programs that are never games, whatever the signs (executable names, lower case).
    pub never: Vec<String>,
}

impl Default for Replay {
    fn default() -> Self {
        Self {
            enabled: false,
            duration_seconds: 180,
            duration_unit: "minutes".into(),
            after_enabled: false,
            after_seconds: 5,
            storage: "auto".into(),
            same_as_video: false,
            video: Video::replay(),
            always: Vec::new(),
            never: Vec::new(),
        }
    }
}

impl Replay {
    /// The seconds recorded after the shortcut (0: the replay stops at the press).
    pub fn after(&self) -> u32 {
        if self.after_enabled { self.after_seconds.clamp(1, 60) } else { 0 }
    }
}

/// Which notifications are shown. Off, none is; the others keep their setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub enabled: bool,
    pub image_saved: bool,
    /// The text read in a zone is on the clipboard.
    pub text_copied: bool,
    pub recording_saved: bool,
    pub replay_saved: bool,
    /// When a game starts: `both` (replay on or off), `on` (only when it runs) or `none`.
    pub game_start: String,
    /// A capture, a recording or a replay failed, a sound source went away, Vixeeny stopped unexpectedly.
    pub failures: bool,
    /// A new version can be installed.
    pub updates: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            image_saved: true,
            text_copied: true,
            recording_saved: true,
            replay_saved: true,
            game_start: "both".into(),
            failures: true,
            updates: true,
        }
    }
}

impl Notifications {
    /// The card when a game starts, the replay on (`true`) or off.
    pub fn game_start(&self, on: bool) -> bool {
        self.enabled && matches!((self.game_start.as_str(), on), ("both", _) | ("on", true))
    }
}

/// A warning when the free space of the disk the files go to falls under a share of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskAlert {
    pub enabled: bool,
    /// 1 to 99.
    pub percent: u8,
}

impl Default for DiskAlert {
    fn default() -> Self {
        Self { enabled: true, percent: 5 }
    }
}

impl DiskAlert {
    /// The share under which it warns, in percent (0: never).
    pub fn threshold(&self) -> u8 {
        if self.enabled { self.percent.clamp(1, 99) } else { 0 }
    }
}

/// The disk alerts: the recording widget's warning sign, and a notification during a replay.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskAlerts {
    pub recordings: DiskAlert,
    pub replays: DiskAlert,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    /// Off: its shortcut is free for other programs.
    pub enabled: bool,
    pub edge: String,
}

impl Default for Overlay {
    fn default() -> Self {
        Self { enabled: true, edge: "right".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scrolling {
    /// The assembled image is cut at this height.
    pub max_height: u32,
    /// Open the assembled image in the editor before saving it.
    pub annotate: bool,
}

impl Default for Scrolling {
    fn default() -> Self {
        Self { max_height: 30_000, annotate: true }
    }
}

/// Upgrades `table` to `current` by applying `migrations` in order.
pub fn migrate_with(table: &mut toml::Table, current: u32, migrations: &[Migration]) -> Result<(), ConfigError> {
    let found = match table.get("schema_version") {
        None => 1,
        Some(v) => {
            v.as_integer().and_then(|n| u32::try_from(n).ok()).filter(|n| *n >= 1).ok_or(ConfigError::BadVersion)?
        }
    };
    if found > current {
        return Err(ConfigError::TooNew { found, supported: current });
    }
    for version in found..current {
        let step = migrations.get(version as usize - 1).ok_or(ConfigError::BadVersion)?;
        step(table);
        table.insert("schema_version".into(), toml::Value::Integer(i64::from(version) + 1));
    }
    Ok(())
}

impl Config {
    /// Parses and migrates a config document. Unknown keys are ignored, missing keys default, and a value that cannot
    /// be read takes its default without the others.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Self::parse(text).map(|(config, _)| config)
    }

    /// The config, and whether some values could not be read.
    fn parse(text: &str) -> Result<(Self, bool), ConfigError> {
        let mut table: toml::Table = toml::from_str(text)?;
        migrate_with(&mut table, SCHEMA_VERSION, MIGRATIONS)?;
        match table.clone().try_into() {
            Ok(config) => Ok((config, false)),
            Err(e) => {
                tracing::warn!("some settings cannot be read, their defaults are used: {e}");
                let mut kept = toml::Table::new();
                salvage(&mut kept, &mut Vec::new(), table);
                Ok((kept.try_into()?, true))
            }
        }
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Loads `path`; a missing file yields the defaults (nothing is written). A file that cannot be read in full is
    /// first copied to `config.toml.bak`, so that what it held survives the next save.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let parsed = Self::parse(&text);
                if !matches!(parsed, Ok((_, false))) {
                    let _ = std::fs::write(path.with_extension("toml.bak"), &text);
                }
                parsed.map(|(config, _)| config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io { path: path.display().to_string(), source }),
        }
    }

    /// Writes the config whole or not at all ([`crate::paths::write_atomic`]).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |source| ConfigError::Io { path: path.display().to_string(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        crate::paths::write_atomic(path, self.to_toml()?.as_bytes()).map_err(io)
    }
}

/// Puts into `kept` (at `path`) every value of `source` that reads, one by one; a table that does not is gone through
/// value by value.
fn salvage(kept: &mut toml::Table, path: &mut Vec<String>, source: toml::Table) {
    /// The table at `path` (kept before its values are).
    fn at<'a>(kept: &'a mut toml::Table, path: &[String]) -> Option<&'a mut toml::Table> {
        path.iter().try_fold(kept, |table, key| table.get_mut(key)?.as_table_mut())
    }
    let reads = |kept: &toml::Table| kept.clone().try_into::<Config>().is_ok();
    for (key, value) in source {
        let Some(table) = at(kept, path) else {
            return;
        };
        table.insert(key.clone(), value.clone());
        if reads(kept) {
            continue;
        }
        let Some(table) = at(kept, path) else {
            return;
        };
        table.remove(&key);
        if let toml::Value::Table(inner) = value {
            table.insert(key.clone(), toml::Table::new().into());
            if !reads(kept) {
                if let Some(table) = at(kept, path) {
                    table.remove(&key);
                }
                continue;
            }
            path.push(key);
            salvage(kept, path, inner);
            path.pop();
        }
    }
}

/// Three-way merge: what `ours` changed from `base`, applied over `theirs` (the file as another program left it since
/// `base` was read). A setting both changed keeps `ours`.
pub fn merge(base: &Config, ours: &Config, theirs: &Config) -> Config {
    fn value(c: &Config) -> Option<toml::Value> {
        toml::Value::try_from(c).ok()
    }
    fn walk(base: &toml::Value, ours: &toml::Value, theirs: &toml::Value) -> toml::Value {
        match (base, ours, theirs) {
            (toml::Value::Table(b), toml::Value::Table(o), toml::Value::Table(t)) => {
                let mut out = t.clone();
                for (key, mine) in o {
                    let merged = match (b.get(key), t.get(key)) {
                        (Some(b), Some(t)) => walk(b, mine, t),
                        // Ours alone has it, or the base did not: what we set wins.
                        _ => mine.clone(),
                    };
                    out.insert(key.clone(), merged);
                }
                toml::Value::Table(out)
            }
            _ if ours != base => ours.clone(),
            _ => theirs.clone(),
        }
    }
    let (Some(b), Some(o), Some(t)) = (value(base), value(ours), value(theirs)) else {
        return ours.clone();
    };
    walk(&b, &o, &t).try_into().unwrap_or_else(|_| ours.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_colours_read_as_codes_or_names() {
        assert_eq!(parse_hex("#0063B1"), Some([0x00, 0x63, 0xb1]));
        assert_eq!(parse_hex("f80"), Some([0xff, 0x88, 0x00]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(hex([0, 99, 177]), "#0063b1");
        let mut image = Image::default();
        assert_eq!(image.backdrop_colour(), None);
        image.backdrop_colour = "blue".into();
        assert_eq!(image.backdrop_colour(), Some([0x00, 0x63, 0xb1]));
        image.backdrop_colour = "#102030".into();
        assert_eq!(image.backdrop_colour(), Some([0x10, 0x20, 0x30]));
        assert_eq!(image.backdrop_gradient().0, [0x65, 0x4e, 0xa3]);
        image.backdrop_gradient = "#000000,#ffffff".into();
        assert_eq!(image.backdrop_gradient(), ([0; 3], [255; 3]));
    }

    #[test]
    fn a_merge_keeps_both_sides_changes() {
        let base = Config::default();
        // The settings window changed the theme; the widget, moved, changed its place.
        let mut ours = base.clone();
        ours.general.theme = "dark".into();
        let mut theirs = base.clone();
        theirs.recording_widget.corner = "custom".into();
        theirs.recording_widget.custom = (0.3, 0.7);
        let merged = merge(&base, &ours, &theirs);
        assert_eq!(merged.general.theme, "dark");
        assert_eq!(merged.recording_widget.corner, "custom");
        assert_eq!(merged.recording_widget.custom, (0.3, 0.7));
        // Both changed the same setting: ours wins.
        ours.recording_widget.corner = "bottom_right".into();
        assert_eq!(merge(&base, &ours, &theirs).recording_widget.corner, "bottom_right");
        // Nothing changed on our side: theirs as it is.
        assert_eq!(merge(&base, &base, &theirs), theirs);
    }

    #[test]
    fn a_missing_empty_or_partial_file_gives_the_defaults() {
        let path = std::env::temp_dir().join("vx-definitely-missing/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
        let cfg = Config::from_toml("[general]\nautostart = false\n").unwrap();
        // A setting of older versions, gone since: left aside.
        assert_eq!(Config::from_toml("[general]\nsounds = true\n").unwrap(), Config::default());
        assert!(!cfg.general.autostart);
        assert_eq!(cfg.general.app_idle_exit_seconds, 30);
        assert_eq!(cfg.hotkeys, Hotkeys::default());
    }

    #[test]
    fn a_value_that_cannot_be_read_takes_its_default_alone() {
        let text = "schema_version = 6\n\
            [general]\nautostart = \"yes\"\nnotifications = false\n\
            [hotkeys]\ncapture_region = [\"F8\"]\nrecord_toggle = \"F9\"\n\
            [image]\nformat = \"png\"\n[image.jpeg]\nquality = \"high\"\n\
            [replay]\nduration_seconds = -5\n";
        let cfg = Config::from_toml(text).unwrap();
        let defaults = Config::default();
        assert_eq!(cfg.general.autostart, defaults.general.autostart);
        // The general switch of then: every card but the games'.
        assert!(!cfg.notifications.image_saved && !cfg.notifications.failures);
        assert!(cfg.notifications.enabled && cfg.notifications.game_start(true));
        assert_eq!(cfg.hotkeys.capture_region, ["F8"]);
        assert_eq!(cfg.hotkeys.record_toggle, defaults.hotkeys.record_toggle);
        assert_eq!(cfg.image.format, "png");
        assert_eq!(cfg.image.jpeg, defaults.image.jpeg);
        assert_eq!(cfg.replay.duration_seconds, defaults.replay.duration_seconds);
    }

    #[test]
    fn a_file_read_in_part_is_kept_aside_first() {
        let dir = std::env::temp_dir().join(format!("vx-config-bak-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let backup = dir.join("config.toml.bak");
        std::fs::write(&path, "[hotkeys]\ncapture_region = [\"F8\"]\n").unwrap();
        Config::load(&path).unwrap();
        assert!(!backup.exists());
        for text in ["[general]\nautostart = 3\n", "not toml ["] {
            std::fs::write(&path, text).unwrap();
            let _ = Config::load(&path);
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), text);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_from_a_newer_version_or_damaged_is_refused() {
        let err = Config::from_toml("schema_version = 99\n").unwrap_err();
        assert!(matches!(err, ConfigError::TooNew { found: 99, .. }));
        for text in ["schema_version = 0\n", "schema_version = \"x\"\n"] {
            assert!(matches!(Config::from_toml(text), Err(ConfigError::BadVersion)));
        }
    }

    #[test]
    fn older_files_record_fragmented_mp4_without_the_cursor() {
        let text = "schema_version = 1\n[profiles.default]\ncontainer = \"mp4_hybrid\"\n\
                    show_cursor = true\n[profiles.mkv]\ncontainer = \"mkv\"\n";
        let cfg = Config::from_toml(text).unwrap();
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
        // Then hybrid (1.0).
        assert_eq!(cfg.video.container, "mp4_hybrid");
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
        let text = "schema_version = 2\n[video]\nprofile = \"Gone\"\n[profiles.default]\nfps = 30\n";
        assert_eq!(Config::from_toml(text).unwrap().video.fps, 30);
        let text = "schema_version = 2\n[video]\nprofile = \"Gone\"\n";
        assert_eq!(Config::from_toml(text).unwrap().video, Video::default());
    }

    #[test]
    fn the_widget_that_hid_when_idle_now_fades() {
        let text = "schema_version = 4\n[recording_widget]\nauto_hide = true\n";
        assert_eq!(Config::from_toml(text).unwrap().recording_widget.display, "fade");
        let text = "schema_version = 4\n[recording_widget]\nauto_hide = false\n";
        assert_eq!(Config::from_toml(text).unwrap().recording_widget.display, "always");
    }

    #[test]
    fn one_naming_becomes_that_of_each_kind() {
        let text = "schema_version = 3\n[paths]\nfilename_template = \"{app}-{date}\"\n\
                    use_foreground_app = false\n";
        let cfg = Config::from_toml(text).unwrap();
        for naming in [&cfg.paths.naming.images, &cfg.paths.naming.videos, &cfg.paths.naming.replays] {
            assert_eq!(naming.template, "{app}-{date}");
        }
        // Without them, the defaults.
        let cfg = Config::from_toml("schema_version = 3\n[paths]\nimages = \"x\"\n").unwrap();
        assert_eq!(cfg.paths.naming, PerKindNaming::default());
    }

    #[test]
    fn a_third_shortcut_is_dropped() {
        let cfg =
            Config::from_toml("schema_version = 5\n[hotkeys]\nopen_settings = [\"Ctrl+1\", \"Ctrl+2\", \"Ctrl+3\"]\n")
                .unwrap();
        assert_eq!(cfg.hotkeys.open_settings, ["Ctrl+1", "Ctrl+2"]);
    }

    #[test]
    fn the_scrolling_capture_gets_its_shortcut_when_free() {
        let read =
            |hotkeys: &str| Config::from_toml(&format!("schema_version = 6\n[hotkeys]\n{hotkeys}")).unwrap().hotkeys;
        // Given its shortcut, then moved to the one of 1.0.
        let h = read("capture_scrolling = []\n");
        assert_eq!(h.capture_scrolling, ["Shift+PrintScreen"]);
        let h = read("capture_scrolling = [\"F7\"]\n");
        assert_eq!(h.capture_scrolling, ["F7"]);
        let h = read("capture_scrolling = []\nopen_settings = [\"ctrl+printscreen\"]\n");
        assert!(h.capture_scrolling.is_empty());
    }

    #[test]
    fn the_former_default_shortcuts_move_off_ctrl_shift_letters() {
        let read =
            |hotkeys: &str| Config::from_toml(&format!("schema_version = 7\n[hotkeys]\n{hotkeys}")).unwrap().hotkeys;
        let h = read(
            "record_toggle = [\"Ctrl+Shift+R\", \"F9\"]\nrecord_pause = [\"Shift+Ctrl+P\"]\n\
             replay_save = [\"Alt+S\"]\n",
        );
        // Moved, then without Shift (1.0).
        assert_eq!(h.record_toggle, ["Ctrl+F9", "F9"]);
        assert_eq!(h.record_pause, ["Ctrl+F10"]);
        // Changed by the user: kept.
        assert_eq!(h.replay_save, ["Alt+S"]);
        // The new one already given to another action: the old one stays.
        let h = read("replay_edit = [\"Ctrl+Shift+E\"]\nopen_settings = [\"Ctrl+Shift+F12\"]\n");
        assert_eq!(h.replay_edit, ["Ctrl+Shift+E"]);
    }

    #[test]
    fn the_former_defaults_move_to_the_new_ones() {
        let read = |text: &str| Config::from_toml(&format!("schema_version = 8\n{text}")).unwrap();
        let cfg = read(
            "[hotkeys]\ncapture_fullscreen = [\"Shift+PrintScreen\"]\n\
             capture_scrolling = [\"Ctrl+PrintScreen\"]\nrecord_toggle = [\"Ctrl+Shift+F9\", \"F9\"]\n\
             replay_save = [\"Alt+S\"]\n\
             [paths]\nimages = \"{pictures}/Vixeeny\"\nvideos = \"D:\\\\Clips\"\n\
             replays = \"{videos}/Vixeeny/Replays\"\n\
             [video]\ncontainer = \"mp4_fragmented\"\n",
        );
        // The screen and the scrolling capture swap theirs.
        assert_eq!(cfg.hotkeys.capture_fullscreen, ["Ctrl+PrintScreen"]);
        assert_eq!(cfg.hotkeys.capture_scrolling, ["Shift+PrintScreen"]);
        assert_eq!(cfg.hotkeys.record_toggle, ["Ctrl+F9", "F9"]);
        assert_eq!(cfg.hotkeys.replay_save, ["Alt+S"]);
        assert_eq!(cfg.paths.images, crate::paths::DEFAULT_IMAGES);
        assert_eq!(cfg.paths.videos, "D:\\Clips");
        assert_eq!(cfg.paths.replays, crate::paths::DEFAULT_REPLAYS);
        assert_eq!(cfg.video.container, "mp4_hybrid");
        // A new one held by an action that keeps it: that move does not happen, nor the one that would land on the
        // shortcut left in place.
        let cfg = read(
            "[hotkeys]\ncapture_fullscreen = [\"Shift+PrintScreen\"]\n\
             capture_scrolling = [\"Ctrl+PrintScreen\"]\nopen_settings = [\"Ctrl+PrintScreen\", \"Ctrl+F9\"]\n\
             record_toggle = [\"Ctrl+Shift+F9\"]\n",
        );
        assert_eq!(cfg.hotkeys.capture_fullscreen, ["Shift+PrintScreen"]);
        assert_eq!(cfg.hotkeys.capture_scrolling, ["Ctrl+PrintScreen"]);
        assert_eq!(cfg.hotkeys.record_toggle, ["Ctrl+Shift+F9"]);
    }

    #[test]
    fn the_replay_starts_light_on_a_hardware_encoder() {
        let cfg = Config::default();
        let replay = cfg.replay_video();
        assert_eq!((replay.encoder.as_str(), replay.auto_codec.as_str()), ("auto", ""));
        assert_eq!((replay.resolution.as_str(), replay.fps, replay.preset.as_str()), ("1080p", 60, "performance"));
        assert_eq!((cfg.replay.duration_seconds, cfg.replay.duration_unit.as_str()), (180, "minutes"));
        assert_eq!(cfg.video.preset, "balanced");
        assert_eq!((replay.audio.codec.as_str(), replay.audio.bitrate_kbps), ("aac", 160));
        assert_eq!((replay.audio.capture.as_str(), replay.audio.routing.as_str()), ("target", "mix_all"));
        // The videos keep theirs, and the replay takes them all, sound included, when it follows them.
        assert_eq!(cfg.video.auto_codec, "");
        let mut cfg = cfg;
        cfg.video.audio.bitrate_kbps = 320;
        cfg.replay.same_as_video = true;
        assert_eq!(cfg.replay_video().audio.bitrate_kbps, 320);
        assert_eq!(cfg.replay_video().resolution, "source");
        // Never the processor: the hardware encoder the settings had, else the best one.
        cfg.video.encoder_kind = "software".into();
        cfg.video.encoder = "libx265".into();
        assert_eq!(cfg.replay_video().encoder, "auto");
        cfg.video.encoder_by_kind.insert("hardware".into(), "nvenc_hevc".into());
        assert_eq!(
            (cfg.replay_video().encoder_kind.as_str(), cfg.replay_video().encoder.as_str()),
            ("hardware", "nvenc_hevc")
        );
        // A file without them gets them too.
        let read =
            Config::from_toml(&format!("schema_version = {SCHEMA_VERSION}\n[replay]\nenabled = true\n")).unwrap();
        assert_eq!(read.replay.video, Video::replay());
    }

    #[test]
    fn the_settings_of_before_1_0_find_their_new_places() {
        let read = |text: &str| Config::from_toml(&format!("schema_version = 9\n{text}")).unwrap();
        let cfg = read(
            "[general]\nnotifications = false\n\
             [video]\nfps = 120\npreset = \"small\"\n[video.audio]\nbitrate_kbps = 320\n\
             [replay]\nnotify = \"on\"\nduration_seconds = 300\nafter_seconds = 10\n\
             [recording_widget]\ndisk_alert_percent = 0\n\
             [paths.naming.images]\ntemplate = \"{app}_{date}_{time}\"\n\
             [paths.naming.videos]\ntemplate = \"{app}-{date}\"\n\
             [hotkeys]\nopen_settings = [\"Ctrl+F7\"]\ncapture_all_monitors = []\nreplay_toggle = []\n",
        );
        let n = &cfg.notifications;
        assert!(n.enabled && !n.image_saved && !n.updates);
        assert!(n.game_start(true) && !n.game_start(false));
        // Five minutes, and ten seconds after the shortcut.
        assert_eq!((cfg.replay.duration_seconds, cfg.replay.duration_unit.as_str()), (300, "minutes"));
        assert_eq!(cfg.replay.after(), 10);
        // The replay keeps recording as it did: with the video settings, as its own.
        assert!(!cfg.replay.same_as_video);
        assert_eq!(cfg.replay.video.fps, 120);
        assert_eq!((cfg.video.preset.as_str(), cfg.replay.video.preset.as_str()), ("balanced", "balanced"));
        assert_eq!(cfg.replay_video().audio.bitrate_kbps, 320);
        assert_eq!(cfg.replay.video.auto_codec, "");
        // The alert was off: off, its share ready for when it is on again.
        assert_eq!(cfg.disk_alert.recordings, DiskAlert { enabled: false, percent: 5 });
        assert_eq!(cfg.disk_alert.replays.threshold(), 0);
        assert_eq!(cfg.paths.naming.images.template, "{app} {date} {time}");
        assert_eq!(cfg.paths.naming.videos.template, "{app}-{date}");
        // The shortcuts given when free.
        assert!(cfg.hotkeys.capture_all_monitors.is_empty());
        assert_eq!(cfg.hotkeys.replay_toggle, ["Ctrl+Shift+F11"]);
        // Every card off then, the games' too.
        let cfg = read("[general]\nnotifications = false\n[replay]\nnotify = \"none\"\nafter_seconds = 0\n");
        assert!(!cfg.notifications.enabled && cfg.notifications.image_saved);
        assert!(!cfg.replay.after_enabled && cfg.replay.after_seconds == 5);
        assert_eq!(read("").notifications, Notifications::default());
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
}
