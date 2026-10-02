// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings model (plan 5.13): every setting is a [`Row`] with a way to read it from a
//! [`Config`] and a validated way to write it. The settings window only draws rows; a change is
//! applied the moment it is made, and each section can be reset to its defaults.
//!
//! No UI and no OS code here, so all of it is tested on any machine.

pub mod profiles;
pub mod shortcuts;

use std::sync::LazyLock;

use vixeeny_common::config::{Config, Profile};
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_encode::registry::{Platform, Registry};

/// The sections of the settings window, in the order of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Gallery,
    General,
    Shortcuts,
    Images,
    Video,
    Audio,
    Replay,
    Folders,
    Ocr,
    Profiles,
    Hardware,
    Updates,
    Integration,
    About,
}

impl Section {
    pub const ALL: [Self; 14] = [
        Self::Gallery,
        Self::General,
        Self::Shortcuts,
        Self::Images,
        Self::Video,
        Self::Audio,
        Self::Replay,
        Self::Folders,
        Self::Ocr,
        Self::Profiles,
        Self::Hardware,
        Self::Updates,
        Self::Integration,
        Self::About,
    ];

    pub const fn title(self) -> Key {
        match self {
            Self::Gallery => Key::SecGallery,
            Self::General => Key::SecGeneral,
            Self::Shortcuts => Key::SecShortcuts,
            Self::Images => Key::SecImages,
            Self::Video => Key::SecVideo,
            Self::Audio => Key::SecAudio,
            Self::Replay => Key::SecReplay,
            Self::Folders => Key::SecFolders,
            Self::Ocr => Key::SecOcr,
            Self::Profiles => Key::SecProfiles,
            Self::Hardware => Key::SecHardware,
            Self::Updates => Key::SecUpdates,
            Self::Integration => Key::SecIntegration,
            Self::About => Key::SecAbout,
        }
    }

    /// Whether the section is a plain list of [`Row`]s (the others have a page of their own).
    pub const fn is_rows(self) -> bool {
        matches!(
            self,
            Self::General
                | Self::Images
                | Self::Video
                | Self::Audio
                | Self::Replay
                | Self::Folders
                | Self::Ocr
                | Self::Updates
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
}

/// One choice of a [`Kind::Choice`] row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opt {
    /// What is stored in the settings file.
    pub value: String,
    /// What the user reads.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Toggle,
    Choice(Vec<Opt>),
    Number {
        min: i64,
        max: i64,
        step: i64,
    },
    Text,
    Folder,
    /// Read-only.
    Info,
}

/// The value was refused (not one of the choices, not a number, empty where it must not be).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

type Get = Box<dyn Fn(&Config) -> Value>;
type Set = Box<dyn Fn(&mut Config, Value) -> Result<(), Invalid>>;

pub struct Row {
    pub id: &'static str,
    pub label: String,
    pub kind: Kind,
    get: Get,
    set: Set,
    enabled: fn(&Config) -> bool,
}

impl Row {
    pub fn value(&self, config: &Config) -> Value {
        (self.get)(config)
    }

    /// Greyed out (a setting that does not apply with the others, like the amount of a split
    /// that is off).
    pub fn enabled(&self, config: &Config) -> bool {
        self.kind != Kind::Info && (self.enabled)(config)
    }

    /// Checks `value` against the kind of the row, then writes it. Numbers are brought into
    /// range and onto the step.
    pub fn apply(&self, config: &mut Config, value: Value) -> Result<(), Invalid> {
        let value = match (&self.kind, value) {
            (Kind::Toggle, v @ Value::Bool(_)) => v,
            (Kind::Choice(options), Value::Text(t)) => {
                if !options.iter().any(|o| o.value == t) {
                    return Err(Invalid);
                }
                Value::Text(t)
            }
            (Kind::Number { min, max, step }, Value::Int(n)) => {
                let step = (*step).max(1);
                let snapped = (n.clamp(*min, *max) - min + step / 2) / step * step + min;
                Value::Int(snapped.clamp(*min, *max))
            }
            (Kind::Text | Kind::Folder, Value::Text(t)) => Value::Text(t.trim().to_owned()),
            _ => return Err(Invalid),
        };
        (self.set)(config, value)
    }
}

/// What the rows need to know about this machine.
#[derive(Debug, Clone)]
pub struct Env {
    pub lang: Lang,
    /// `(id, name)` of the encoders of this platform.
    pub encoders: Vec<(String, String)>,
    pub version: String,
}

static REGISTRY: LazyLock<Option<Registry>> = LazyLock::new(|| Registry::builtin().ok());

impl Env {
    pub fn new(lang: Lang, version: &str) -> Self {
        let encoders = REGISTRY
            .as_ref()
            .map(|r| {
                r.for_platform(Platform::current())
                    .map(|e| (e.id.clone(), e.display_name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            lang,
            encoders,
            version: version.to_owned(),
        }
    }

    fn t(&self, key: Key) -> String {
        tr(key, self.lang).to_owned()
    }
}

fn opt(value: &str, label: impl Into<String>) -> Opt {
    Opt {
        value: value.to_owned(),
        label: label.into(),
    }
}

fn raw(values: &[&str]) -> Vec<Opt> {
    values.iter().map(|v| opt(v, *v)).collect()
}

fn always(_: &Config) -> bool {
    true
}

fn toggle(
    id: &'static str,
    label: String,
    get: fn(&Config) -> bool,
    set: fn(&mut Config, bool),
) -> Row {
    Row {
        id,
        label,
        kind: Kind::Toggle,
        get: Box::new(move |c| Value::Bool(get(c))),
        set: Box::new(move |c, v| match v {
            Value::Bool(b) => {
                set(c, b);
                Ok(())
            }
            _ => Err(Invalid),
        }),
        enabled: always,
    }
}

fn number(
    id: &'static str,
    label: String,
    (min, max, step): (i64, i64, i64),
    get: fn(&Config) -> i64,
    set: fn(&mut Config, i64),
) -> Row {
    Row {
        id,
        label,
        kind: Kind::Number { min, max, step },
        get: Box::new(move |c| Value::Int(get(c))),
        set: Box::new(move |c, v| match v {
            Value::Int(n) => {
                set(c, n);
                Ok(())
            }
            _ => Err(Invalid),
        }),
        enabled: always,
    }
}

fn text_like(
    kind: Kind,
    id: &'static str,
    label: String,
    get: fn(&Config) -> String,
    set: fn(&mut Config, String),
) -> Row {
    Row {
        id,
        label,
        kind,
        get: Box::new(move |c| Value::Text(get(c))),
        set: Box::new(move |c, v| match v {
            Value::Text(t) => {
                set(c, t);
                Ok(())
            }
            _ => Err(Invalid),
        }),
        enabled: always,
    }
}

fn choice(
    id: &'static str,
    label: String,
    options: Vec<Opt>,
    get: fn(&Config) -> String,
    set: fn(&mut Config, String),
) -> Row {
    text_like(Kind::Choice(options), id, label, get, set)
}

fn text(
    id: &'static str,
    label: String,
    get: fn(&Config) -> String,
    set: fn(&mut Config, String),
) -> Row {
    text_like(Kind::Text, id, label, get, set)
}

fn folder(
    id: &'static str,
    label: String,
    get: fn(&Config) -> String,
    set: fn(&mut Config, String),
) -> Row {
    text_like(Kind::Folder, id, label, get, set)
}

fn when(mut row: Row, enabled: fn(&Config) -> bool) -> Row {
    row.enabled = enabled;
    row
}

fn info(id: &'static str, label: String, value: String) -> Row {
    Row {
        id,
        label,
        kind: Kind::Info,
        get: Box::new(move |_| Value::Text(value.clone())),
        set: Box::new(|_, _| Err(Invalid)),
        enabled: always,
    }
}

fn info_of(id: &'static str, label: String, get: fn(&Config) -> String) -> Row {
    Row {
        id,
        label,
        kind: Kind::Info,
        get: Box::new(move |c| Value::Text(get(c))),
        set: Box::new(|_, _| Err(Invalid)),
        enabled: always,
    }
}

/// The profile the video and audio rows edit: `video.profile`, created on first write.
trait Current {
    fn cur(&self) -> Profile;
    fn cur_mut(&mut self) -> &mut Profile;
}

impl Current for Config {
    fn cur(&self) -> Profile {
        self.profiles
            .get(&self.video.profile)
            .cloned()
            .unwrap_or_default()
    }

    fn cur_mut(&mut self) -> &mut Profile {
        let name = self.video.profile.clone();
        self.profiles.entry(name).or_default()
    }
}

/// `off`, `size:<MB>` or `duration:<minutes>` → (kind, amount).
fn split_parts(mode: &str) -> (&str, i64) {
    match mode.split_once(':') {
        Some((kind @ ("size" | "duration"), n)) => (kind, n.parse().unwrap_or(0)),
        _ => ("off", 0),
    }
}

fn split_join(kind: &str, amount: i64) -> String {
    match kind {
        "size" | "duration" => format!("{kind}:{}", amount.max(1)),
        _ => "off".into(),
    }
}

fn list(items: &[String]) -> String {
    items.join(", ")
}

fn unlist(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn app_names_text(c: &Config) -> String {
    c.paths
        .app_names
        .iter()
        .map(|(exe, name)| format!("{exe}={name}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_app_names(text: &str) -> std::collections::BTreeMap<String, String> {
    text.split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(exe, name)| (exe.trim().to_owned(), name.trim().to_owned()))
        .filter(|(exe, name)| !exe.is_empty() && !name.is_empty())
        .collect()
}

/// The rows of a section (empty for the sections that have a page of their own).
#[allow(clippy::too_many_lines)]
pub fn rows(section: Section, env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    match section {
        Section::General => vec![
            choice(
                "language",
                t(Key::SetLanguage),
                vec![
                    opt("auto", t(Key::SetLangAuto)),
                    opt("fr", "Français"),
                    opt("en", "English"),
                ],
                |c| c.general.language.clone(),
                |c, v| c.general.language = v,
            ),
            choice(
                "theme",
                t(Key::SetTheme),
                vec![
                    opt("system", t(Key::SetThemeSystem)),
                    opt("light", t(Key::SetThemeLight)),
                    opt("dark", t(Key::SetThemeDark)),
                ],
                |c| c.general.theme.clone(),
                |c, v| c.general.theme = v,
            ),
            toggle(
                "autostart",
                t(Key::SetAutostart),
                |c| c.general.autostart,
                |c, v| c.general.autostart = v,
            ),
            number(
                "idle_exit",
                t(Key::SetIdleExit),
                (0, 600, 5),
                |c| i64::from(c.general.app_idle_exit_seconds),
                |c, v| c.general.app_idle_exit_seconds = v as u32,
            ),
            toggle(
                "sounds",
                t(Key::SetSounds),
                |c| c.general.sounds,
                |c, v| c.general.sounds = v,
            ),
            toggle(
                "notifications",
                t(Key::SetNotifications),
                |c| c.general.notifications,
                |c, v| c.general.notifications = v,
            ),
            choice(
                "overlay_edge",
                t(Key::SetOverlayEdge),
                vec![
                    opt("left", t(Key::SetEdgeLeft)),
                    opt("right", t(Key::SetEdgeRight)),
                    opt("top", t(Key::SetEdgeTop)),
                    opt("bottom", t(Key::SetEdgeBottom)),
                ],
                |c| c.overlay.edge.clone(),
                |c, v| c.overlay.edge = v,
            ),
            toggle(
                "widget",
                t(Key::SetWidget),
                |c| c.recording_widget.enabled,
                |c, v| c.recording_widget.enabled = v,
            ),
            when(
                choice(
                    "widget_corner",
                    t(Key::SetWidgetCorner),
                    vec![
                        opt("top_left", t(Key::SetCornerTl)),
                        opt("top_right", t(Key::SetCornerTr)),
                        opt("bottom_left", t(Key::SetCornerBl)),
                        opt("bottom_right", t(Key::SetCornerBr)),
                    ],
                    |c| c.recording_widget.corner.clone(),
                    |c, v| c.recording_widget.corner = v,
                ),
                |c| c.recording_widget.enabled,
            ),
            when(
                toggle(
                    "widget_hide",
                    t(Key::SetWidgetHide),
                    |c| c.recording_widget.auto_hide,
                    |c, v| c.recording_widget.auto_hide = v,
                ),
                |c| c.recording_widget.enabled,
            ),
        ],
        Section::Images => vec![
            choice(
                "image_format",
                t(Key::SetImageFormat),
                raw(&["png", "jpeg", "webp", "avif", "jxl"]),
                |c| c.image.format.clone(),
                |c, v| c.image.format = v,
            ),
            toggle(
                "image_cursor",
                t(Key::SetShowCursor),
                |c| c.image.show_cursor,
                |c, v| c.image.show_cursor = v,
            ),
            choice(
                "image_hdr",
                t(Key::SetHdr),
                vec![
                    opt("tonemap_sdr", t(Key::SetHdrTonemap)),
                    opt("keep_hdr", t(Key::SetHdrKeep)),
                ],
                |c| c.image.hdr.clone(),
                |c, v| c.image.hdr = v,
            ),
            toggle(
                "clipboard",
                t(Key::SetCopyClipboard),
                |c| c.image.copy_to_clipboard,
                |c, v| c.image.copy_to_clipboard = v,
            ),
            when(
                number(
                    "jpeg_quality",
                    t(Key::SetJpegQuality),
                    (1, 100, 1),
                    |c| i64::from(c.image.jpeg.quality),
                    |c, v| c.image.jpeg.quality = v as u8,
                ),
                |c| c.image.format == "jpeg",
            ),
            when(
                choice(
                    "jpeg_chroma",
                    t(Key::SetJpegChroma),
                    raw(&["444", "420"]),
                    |c| c.image.jpeg.chroma.clone(),
                    |c, v| c.image.jpeg.chroma = v,
                ),
                |c| c.image.format == "jpeg",
            ),
            when(
                number(
                    "avif_quality",
                    t(Key::SetAvifQuality),
                    (0, 100, 1),
                    |c| i64::from(c.image.avif.quality),
                    |c, v| c.image.avif.quality = v as u8,
                ),
                |c| c.image.format == "avif",
            ),
            when(
                choice(
                    "avif_depth",
                    t(Key::SetAvifDepth),
                    raw(&["8", "10"]),
                    |c| c.image.avif.depth.to_string(),
                    |c, v| c.image.avif.depth = v.parse().unwrap_or(10),
                ),
                |c| c.image.format == "avif",
            ),
            number(
                "dim",
                t(Key::SetDim),
                (0, 90, 5),
                |c| i64::from(c.editor.dim_percent),
                |c, v| c.editor.dim_percent = v as u8,
            ),
            number(
                "scroll_max",
                t(Key::SetScrollMax),
                (1_000, 100_000, 1_000),
                |c| i64::from(c.scrolling.max_height),
                |c, v| c.scrolling.max_height = v as u32,
            ),
        ],
        Section::Video => {
            let mut encoders = vec![opt("auto", t(Key::SetEncoderAuto))];
            encoders.extend(env.encoders.iter().map(|(id, name)| opt(id, name.as_str())));
            vec![
                info_of("editing", t(Key::SetEditing), |c| c.video.profile.clone()),
                choice(
                    "encoder",
                    t(Key::SetEncoder),
                    encoders,
                    |c| c.cur().encoder,
                    |c, v| c.cur_mut().encoder = v,
                ),
                choice(
                    "container",
                    t(Key::SetContainer),
                    raw(&["mp4_hybrid", "mp4_fragmented", "mkv", "webm"]),
                    |c| c.cur().container,
                    |c, v| c.cur_mut().container = v,
                ),
                choice(
                    "resolution",
                    t(Key::SetResolution),
                    vec![
                        opt("source", t(Key::SetResSource)),
                        opt("2160p", "2160p"),
                        opt("1440p", "1440p"),
                        opt("1080p", "1080p"),
                        opt("720p", "720p"),
                        opt("480p", "480p"),
                    ],
                    |c| c.cur().resolution,
                    |c, v| c.cur_mut().resolution = v,
                ),
                choice(
                    "fps",
                    t(Key::SetFps),
                    raw(&["24", "30", "60", "90", "120", "144", "240"]),
                    |c| c.cur().fps.to_string(),
                    |c, v| c.cur_mut().fps = v.parse().unwrap_or(60),
                ),
                choice(
                    "depth",
                    t(Key::SetDepth),
                    raw(&["8", "10"]),
                    |c| c.cur().depth.to_string(),
                    |c, v| c.cur_mut().depth = v.parse().unwrap_or(8),
                ),
                choice(
                    "chroma",
                    t(Key::SetChroma),
                    raw(&["420", "422", "444"]),
                    |c| c.cur().chroma,
                    |c, v| c.cur_mut().chroma = v,
                ),
                choice(
                    "hdr",
                    t(Key::SetHdr),
                    vec![
                        opt("tonemap_sdr", t(Key::SetHdrTonemap)),
                        opt("keep_hdr", t(Key::SetHdrKeep)),
                    ],
                    |c| c.cur().hdr,
                    |c, v| c.cur_mut().hdr = v,
                ),
                choice(
                    "preset",
                    t(Key::SetPreset),
                    vec![
                        opt("quality", t(Key::SetPresetQuality)),
                        opt("balanced", t(Key::SetPresetBalanced)),
                        opt("performance", t(Key::SetPresetPerformance)),
                        opt("small", t(Key::SetPresetSmall)),
                    ],
                    |c| c.cur().preset,
                    |c, v| c.cur_mut().preset = v,
                ),
                toggle(
                    "video_cursor",
                    t(Key::SetShowCursor),
                    |c| c.cur().show_cursor,
                    |c, v| c.cur_mut().show_cursor = v,
                ),
                toggle(
                    "vfr",
                    t(Key::SetVfr),
                    |c| c.cur().vfr,
                    |c, v| c.cur_mut().vfr = v,
                ),
                choice(
                    "split",
                    t(Key::SetSplit),
                    vec![
                        opt("off", t(Key::SetSplitOff)),
                        opt("size", t(Key::SetSplitSize)),
                        opt("duration", t(Key::SetSplitDuration)),
                    ],
                    |c| split_parts(&c.cur().split.mode).0.to_owned(),
                    |c, v| {
                        let amount = split_parts(&c.cur().split.mode).1;
                        let amount = if amount > 0 {
                            amount
                        } else if v == "size" {
                            2_048
                        } else {
                            10
                        };
                        c.cur_mut().split.mode = split_join(&v, amount);
                    },
                ),
                when(
                    number(
                        "split_amount",
                        t(Key::SetSplitSizeMb),
                        (1, 1_000_000, 1),
                        |c| split_parts(&c.cur().split.mode).1,
                        |c, v| {
                            let kind = split_parts(&c.cur().split.mode).0.to_owned();
                            c.cur_mut().split.mode = split_join(&kind, v);
                        },
                    ),
                    |c| split_parts(&c.cur().split.mode).0 != "off",
                ),
            ]
        }
        Section::Audio => vec![
            choice(
                "audio_routing",
                t(Key::SetAudioRouting),
                vec![
                    opt("one_track_per_source", t(Key::SetRouteEach)),
                    opt("mix_all", t(Key::SetRouteMix)),
                    opt("advanced", t(Key::SetRouteAdvanced)),
                ],
                |c| c.cur().audio.routing,
                |c, v| c.cur_mut().audio.routing = v,
            ),
            text(
                "audio_sources",
                t(Key::SetAudioSources),
                |c| list(&c.cur().audio.sources),
                |c, v| c.cur_mut().audio.sources = unlist(&v),
            ),
            choice(
                "audio_codec",
                t(Key::SetAudioCodec),
                raw(&["auto", "aac", "opus", "flac", "pcm16", "pcm24"]),
                |c| c.cur().audio.codec,
                |c, v| c.cur_mut().audio.codec = v,
            ),
            number(
                "audio_bitrate",
                t(Key::SetAudioBitrate),
                (32, 512, 16),
                |c| i64::from(c.cur().audio.bitrate_kbps),
                |c, v| c.cur_mut().audio.bitrate_kbps = v as u32,
            ),
            toggle(
                "audio_vbr",
                t(Key::SetAudioVbr),
                |c| c.cur().audio.vbr,
                |c, v| c.cur_mut().audio.vbr = v,
            ),
        ],
        Section::Replay => {
            let mut profiles = vec![opt("", t(Key::SetSameAsRecording))];
            profiles.extend(config.profiles.keys().map(|name| opt(name, name.as_str())));
            vec![
                toggle(
                    "replay_start",
                    t(Key::SetReplayStart),
                    |c| c.replay.enabled_on_start,
                    |c, v| c.replay.enabled_on_start = v,
                ),
                number(
                    "replay_duration",
                    t(Key::SetReplayDuration),
                    (5, 1_200, 5),
                    |c| i64::from(c.replay.duration_seconds),
                    |c, v| c.replay.duration_seconds = v as u32,
                ),
                choice(
                    "replay_profile",
                    t(Key::SetReplayProfile),
                    profiles,
                    |c| c.replay.profile.clone(),
                    |c, v| c.replay.profile = v,
                ),
            ]
        }
        Section::Folders => vec![
            folder(
                "dir_images",
                t(Key::SetDirImages),
                |c| c.paths.images.clone(),
                |c, v| c.paths.images = v,
            ),
            folder(
                "dir_videos",
                t(Key::SetDirVideos),
                |c| c.paths.videos.clone(),
                |c, v| c.paths.videos = v,
            ),
            folder(
                "dir_replays",
                t(Key::SetDirReplays),
                |c| c.paths.replays.clone(),
                |c, v| c.paths.replays = v,
            ),
            text(
                "template",
                t(Key::SetTemplate),
                |c| c.paths.filename_template.clone(),
                |c, v| {
                    if !v.is_empty() {
                        c.paths.filename_template = v;
                    }
                },
            ),
            toggle(
                "sub_images",
                t(Key::SetSubImages),
                |c| c.paths.per_app_subfolder.images,
                |c, v| c.paths.per_app_subfolder.images = v,
            ),
            toggle(
                "sub_videos",
                t(Key::SetSubVideos),
                |c| c.paths.per_app_subfolder.videos,
                |c, v| c.paths.per_app_subfolder.videos = v,
            ),
            toggle(
                "sub_replays",
                t(Key::SetSubReplays),
                |c| c.paths.per_app_subfolder.replays,
                |c, v| c.paths.per_app_subfolder.replays = v,
            ),
            toggle(
                "foreground_app",
                t(Key::SetForegroundApp),
                |c| c.paths.use_foreground_app,
                |c, v| c.paths.use_foreground_app = v,
            ),
            text("app_names", t(Key::SetAppNames), app_names_text, |c, v| {
                c.paths.app_names = parse_app_names(&v)
            }),
        ],
        Section::Ocr => vec![text(
            "ocr_languages",
            t(Key::SetOcrLanguages),
            |c| list(&c.ocr.languages),
            |c, v| {
                let languages = unlist(&v);
                c.ocr.languages = if languages.is_empty() {
                    vec!["auto".into()]
                } else {
                    languages
                };
            },
        )],
        Section::Updates => vec![
            toggle(
                "check_updates",
                t(Key::SetCheckUpdates),
                |c| c.general.check_updates,
                |c, v| c.general.check_updates = v,
            ),
            info("version", t(Key::SetVersion), env.version.clone()),
        ],
        _ => Vec::new(),
    }
}

/// Sets every row of `section` back to its default. The shortcuts and profile pages reset
/// through their own modules.
pub fn reset(section: Section, env: &Env, config: &mut Config) {
    let defaults = Config::default();
    for row in rows(section, env, config) {
        if row.kind == Kind::Info {
            continue;
        }
        let value = row.value(&defaults);
        let _ = row.apply(config, value);
    }
    if section == Section::Shortcuts {
        config.hotkeys = defaults.hotkeys;
    }
}

/// Things wrong with the current video settings, for the user to read (empty when fine).
pub fn video_problems(config: &Config, source: (u32, u32)) -> Vec<String> {
    use vixeeny_encode::validate::{Context, Severity, validate};
    let Some(registry) = REGISTRY.as_ref() else {
        return Vec::new();
    };
    let ctx = Context {
        registry,
        platform: Platform::current(),
        source,
        probe: None,
    };
    validate(&config.cur(), &ctx)
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| format!("{:?}", i.kind))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::new(Lang::En, "1.2.3")
    }

    fn row<'a>(rows: &'a [Row], id: &str) -> &'a Row {
        rows.iter().find(|r| r.id == id).unwrap()
    }

    #[test]
    fn every_section_with_rows_has_rows_and_the_others_have_none() {
        let config = Config::default();
        for section in Section::ALL {
            let n = rows(section, &env(), &config).len();
            assert_eq!(n > 0, section.is_rows(), "{section:?}");
        }
    }

    #[test]
    fn row_ids_are_unique_in_a_section_and_every_row_reads_back_what_it_writes() {
        let config = Config::default();
        for section in Section::ALL {
            let rows = rows(section, &env(), &config);
            let mut ids: Vec<_> = rows.iter().map(|r| r.id).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), rows.len(), "{section:?}");
            for row in rows.iter().filter(|r| r.kind != Kind::Info) {
                let mut copy = config.clone();
                let value = row.value(&config);
                row.apply(&mut copy, value.clone())
                    .unwrap_or_else(|_| panic!("{} refuses its own value {value:?}", row.id));
                assert_eq!(copy, config, "{} changed the settings by itself", row.id);
            }
        }
    }

    #[test]
    fn values_are_checked_against_the_kind_of_the_row() {
        let mut c = Config::default();
        let rows_ = rows(Section::General, &env(), &c);
        let theme = row(&rows_, "theme");
        assert_eq!(theme.apply(&mut c, Value::Text("dark".into())), Ok(()));
        assert_eq!(c.general.theme, "dark");
        assert_eq!(
            theme.apply(&mut c, Value::Text("purple".into())),
            Err(Invalid)
        );
        assert_eq!(theme.apply(&mut c, Value::Bool(true)), Err(Invalid));
        assert_eq!(c.general.theme, "dark");
        // Numbers are clamped and put on the step.
        let idle = row(&rows_, "idle_exit");
        idle.apply(&mut c, Value::Int(47)).unwrap();
        assert_eq!(c.general.app_idle_exit_seconds, 45);
        idle.apply(&mut c, Value::Int(100_000)).unwrap();
        assert_eq!(c.general.app_idle_exit_seconds, 600);
        idle.apply(&mut c, Value::Int(-5)).unwrap();
        assert_eq!(c.general.app_idle_exit_seconds, 0);
    }

    #[test]
    fn dependent_rows_are_greyed_out_when_they_do_not_apply() {
        let mut c = Config::default();
        let images = rows(Section::Images, &env(), &c);
        assert!(!row(&images, "jpeg_quality").enabled(&c));
        c.image.format = "jpeg".into();
        assert!(row(&images, "jpeg_quality").enabled(&c));
        assert!(!row(&images, "avif_depth").enabled(&c));
        let general = rows(Section::General, &env(), &c);
        assert!(row(&general, "widget_corner").enabled(&c));
        c.recording_widget.enabled = false;
        assert!(!row(&general, "widget_corner").enabled(&c));
    }

    #[test]
    fn video_rows_edit_the_current_profile_only() {
        let mut c = Config::default();
        c.profiles.insert("default".into(), Profile::default());
        c.profiles.insert("Tuto".into(), Profile::default());
        c.video.profile = "Tuto".into();
        let video = rows(Section::Video, &env(), &c);
        row(&video, "fps")
            .apply(&mut c, Value::Text("30".into()))
            .unwrap();
        row(&video, "container")
            .apply(&mut c, Value::Text("mkv".into()))
            .unwrap();
        assert_eq!(
            (
                c.profiles["Tuto"].fps,
                c.profiles["Tuto"].container.as_str()
            ),
            (30, "mkv")
        );
        assert_eq!(c.profiles["default"], Profile::default());
        // A profile that does not exist yet is created on the first write.
        c.video.profile = "Nouveau".into();
        row(&video, "preset")
            .apply(&mut c, Value::Text("small".into()))
            .unwrap();
        assert_eq!(c.profiles["Nouveau"].preset, "small");
    }

    #[test]
    fn the_split_is_a_kind_and_an_amount() {
        let mut c = Config::default();
        let video = rows(Section::Video, &env(), &c);
        let (kind, amount) = (row(&video, "split"), row(&video, "split_amount"));
        assert!(!amount.enabled(&c));
        kind.apply(&mut c, Value::Text("size".into())).unwrap();
        assert_eq!(c.cur().split.mode, "size:2048");
        amount.apply(&mut c, Value::Int(4096)).unwrap();
        assert_eq!(c.cur().split.mode, "size:4096");
        kind.apply(&mut c, Value::Text("duration".into())).unwrap();
        assert_eq!(c.cur().split.mode, "duration:4096");
        assert_eq!(kind.value(&c), Value::Text("duration".into()));
        kind.apply(&mut c, Value::Text("off".into())).unwrap();
        assert_eq!(c.cur().split.mode, "off");
    }

    #[test]
    fn lists_and_tables_are_typed_as_text() {
        let mut c = Config::default();
        let folders = rows(Section::Folders, &env(), &c);
        row(&folders, "app_names")
            .apply(
                &mut c,
                Value::Text(" game.exe = Mon Jeu ;bad; x.exe=X ;=nothing".into()),
            )
            .unwrap();
        assert_eq!(c.paths.app_names.len(), 2);
        assert_eq!(c.paths.app_names["game.exe"], "Mon Jeu");
        assert_eq!(
            row(&folders, "app_names").value(&c),
            Value::Text("game.exe=Mon Jeu; x.exe=X".into())
        );
        let ocr = rows(Section::Ocr, &env(), &c);
        row(&ocr, "ocr_languages")
            .apply(&mut c, Value::Text("fr, en ,, ja".into()))
            .unwrap();
        assert_eq!(c.ocr.languages, ["fr", "en", "ja"]);
        row(&ocr, "ocr_languages")
            .apply(&mut c, Value::Text(" ".into()))
            .unwrap();
        assert_eq!(c.ocr.languages, ["auto"]);
        // The template cannot be emptied.
        row(&folders, "template")
            .apply(&mut c, Value::Text(String::new()))
            .unwrap();
        assert_eq!(c.paths.filename_template, "{app}_{date}_{time}");
    }

    #[test]
    fn the_replay_can_follow_any_profile() {
        let mut c = Config::default();
        c.profiles.insert("Jeu".into(), Profile::default());
        let replay = rows(Section::Replay, &env(), &c);
        let r = row(&replay, "replay_profile");
        let Kind::Choice(options) = &r.kind else {
            panic!()
        };
        assert_eq!(options[0].value, "");
        assert!(options.iter().any(|o| o.value == "Jeu"));
        r.apply(&mut c, Value::Text("Jeu".into())).unwrap();
        assert_eq!(c.replay.profile, "Jeu");
        row(&replay, "replay_duration")
            .apply(&mut c, Value::Int(33))
            .unwrap();
        assert_eq!(c.replay.duration_seconds, 35);
    }

    #[test]
    fn a_section_resets_to_its_defaults_and_leaves_the_others_alone() {
        let mut c = Config::default();
        c.general.theme = "dark".into();
        c.general.sounds = false;
        c.overlay.edge = "left".into();
        c.image.format = "jpeg".into();
        c.hotkeys.capture_region = vec!["F8".into()];
        reset(Section::General, &env(), &mut c);
        assert_eq!(c.general, Config::default().general);
        assert_eq!(c.overlay.edge, "right");
        assert_eq!(c.image.format, "jpeg");
        reset(Section::Shortcuts, &env(), &mut c);
        assert_eq!(c.hotkeys, Config::default().hotkeys);
    }

    #[test]
    fn labels_follow_the_language() {
        let c = Config::default();
        let fr = Env::new(Lang::Fr, "1");
        let en = env();
        assert_eq!(rows(Section::General, &fr, &c)[0].label, "Langue");
        assert_eq!(rows(Section::General, &en, &c)[0].label, "Language");
        assert!(!fr.encoders.is_empty() || cfg!(not(any(windows, unix))));
    }

    #[test]
    fn bad_video_settings_are_reported() {
        let mut c = Config::default();
        assert!(video_problems(&c, (1920, 1080)).is_empty());
        c.profiles.insert("default".into(), Profile::default());
        c.profiles.get_mut("default").unwrap().container = "avi".into();
        assert!(!video_problems(&c, (1920, 1080)).is_empty());
    }
}
