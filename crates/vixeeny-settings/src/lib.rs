// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings model (plan 5.13): every setting is a [`Row`] with a way to read it from a
//! [`Config`] and a validated way to write it. The settings window only draws rows; a change is
//! applied the moment it is made, and each section can be reset to its defaults.
//!
//! No UI and no OS code here, so all of it is tested on any machine.

pub mod encoders;
mod pages;
pub mod profiles;
pub mod shortcuts;

use vixeeny_common::config::{Config, Profile};
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_encode::probe::ProbeResult;

pub use encoders::{EncoderInfo, param_label};

/// The sections of the settings window, in the order of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Gallery,
    General,
    Shortcuts,
    /// Screenshots: format, capture options, folder, text recognition.
    Capture,
    /// Recording: profile, encoder, video, file splitting, folder, replay.
    Video,
    Audio,
    /// Version, updates, links.
    About,
}

impl Section {
    pub const ALL: [Self; 7] = [
        Self::Gallery,
        Self::General,
        Self::Shortcuts,
        Self::Capture,
        Self::Video,
        Self::Audio,
        Self::About,
    ];

    pub const fn title(self) -> Key {
        match self {
            Self::Gallery => Key::SecGallery,
            Self::General => Key::SecGeneral,
            Self::Shortcuts => Key::SecShortcuts,
            Self::Capture => Key::SecCapture,
            Self::Video => Key::SecVideo,
            Self::Audio => Key::SecAudio,
            Self::About => Key::SecAbout,
        }
    }

    /// Whether the section is made of [`Row`]s (the gallery and the shortcuts have a page of
    /// their own).
    pub const fn is_rows(self) -> bool {
        !matches!(self, Self::Gallery | Self::Shortcuts)
    }

    /// Whether "Reset" makes sense on the page.
    pub const fn can_reset(self) -> bool {
        !matches!(self, Self::Gallery | Self::About)
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
    /// A drop-down list.
    Choice(Vec<Opt>),
    /// Two or three choices side by side.
    Segmented(Vec<Opt>),
    /// The recording profile: a choice among the profiles, with buttons to manage them.
    Profile(Vec<String>),
    Number {
        min: i64,
        max: i64,
        step: i64,
    },
    /// A number on a slider.
    Slider {
        min: i64,
        max: i64,
        step: i64,
    },
    Text,
    Folder,
    /// Read-only.
    Info,
    /// A title that groups the rows under it; nothing to edit.
    Header,
}

/// The value was refused (not one of the choices, not a number, empty where it must not be).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

type Get = Box<dyn Fn(&Config) -> Value>;
type Set = Box<dyn Fn(&mut Config, Value) -> Result<(), Invalid>>;
type Enabled = Box<dyn Fn(&Config) -> bool>;

pub struct Row {
    pub id: String,
    pub label: String,
    /// A line of explanation under the label (empty = none).
    pub hint: String,
    pub kind: Kind,
    get: Get,
    set: Set,
    enabled: Enabled,
}

impl Row {
    pub fn value(&self, config: &Config) -> Value {
        (self.get)(config)
    }

    /// Greyed out (a setting that does not apply with the others, like the amount of a split
    /// that is off).
    pub fn enabled(&self, config: &Config) -> bool {
        !matches!(self.kind, Kind::Info | Kind::Header) && (self.enabled)(config)
    }

    /// Checks `value` against the kind of the row, then writes it. Numbers are brought into
    /// range and onto the step.
    pub fn apply(&self, config: &mut Config, value: Value) -> Result<(), Invalid> {
        let value = match (&self.kind, value) {
            (Kind::Toggle, v @ Value::Bool(_)) => v,
            (Kind::Choice(options) | Kind::Segmented(options), Value::Text(t)) => {
                if !options.iter().any(|o| o.value == t) {
                    return Err(Invalid);
                }
                Value::Text(t)
            }
            (Kind::Profile(names), Value::Text(t)) => {
                if !names.contains(&t) {
                    return Err(Invalid);
                }
                Value::Text(t)
            }
            (Kind::Number { min, max, step } | Kind::Slider { min, max, step }, Value::Int(n)) => {
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

/// An output device, microphone or program that can be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEntry {
    /// The device id, or the executable name of a program.
    pub id: String,
    pub name: String,
}

/// What the audio page offers to record (filled by the host, which asks the OS).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioDevices {
    pub outputs: Vec<AudioEntry>,
    pub inputs: Vec<AudioEntry>,
    pub programs: Vec<AudioEntry>,
}

/// What the rows need to know about this machine.
#[derive(Debug, Clone)]
pub struct Env {
    pub lang: Lang,
    /// The encoders of this platform, best first within each kind.
    pub encoders: Vec<EncoderInfo>,
    /// The hardware probe, once it has run.
    pub probe: Option<ProbeResult>,
    /// The probe is running (the hardware encoders are not known yet).
    pub detecting: bool,
    pub audio: AudioDevices,
    pub version: String,
    pub display: Display,
}

/// What the monitors can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Display {
    /// Windows shows HDR ("Use HDR") on at least one monitor.
    pub hdr: bool,
    /// The highest refresh rate among the monitors, in Hz.
    pub max_refresh: u32,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            hdr: false,
            max_refresh: 60,
        }
    }
}

impl Env {
    pub fn new(lang: Lang, version: &str) -> Self {
        Self {
            lang,
            encoders: encoders::all(),
            probe: None,
            detecting: false,
            audio: AudioDevices::default(),
            version: version.to_owned(),
            display: Display::default(),
        }
    }

    pub const fn with_display(mut self, display: Display) -> Self {
        self.display = display;
        self
    }

    pub fn with_probe(mut self, probe: Option<ProbeResult>, detecting: bool) -> Self {
        self.probe = probe;
        self.detecting = detecting;
        self
    }

    pub fn with_audio(mut self, audio: AudioDevices) -> Self {
        self.audio = audio;
        self
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

fn always(_: &Config) -> bool {
    true
}

fn row(id: impl Into<String>, label: String, kind: Kind, get: Get, set: Set) -> Row {
    Row {
        id: id.into(),
        label,
        hint: String::new(),
        kind,
        get,
        set,
        enabled: Box::new(always),
    }
}

fn toggle(
    id: &str,
    label: String,
    get: impl Fn(&Config) -> bool + 'static,
    set: fn(&mut Config, bool),
) -> Row {
    row(
        id,
        label,
        Kind::Toggle,
        Box::new(move |c| Value::Bool(get(c))),
        Box::new(move |c, v| match v {
            Value::Bool(b) => {
                set(c, b);
                Ok(())
            }
            _ => Err(Invalid),
        }),
    )
}

fn number(
    id: &str,
    label: String,
    (min, max, step): (i64, i64, i64),
    get: fn(&Config) -> i64,
    set: fn(&mut Config, i64),
) -> Row {
    row(
        id,
        label,
        Kind::Number { min, max, step },
        Box::new(move |c| Value::Int(get(c))),
        Box::new(move |c, v| match v {
            Value::Int(n) => {
                set(c, n);
                Ok(())
            }
            _ => Err(Invalid),
        }),
    )
}

fn slider(
    id: &str,
    label: String,
    (min, max, step): (i64, i64, i64),
    get: fn(&Config) -> i64,
    set: fn(&mut Config, i64),
) -> Row {
    let mut r = number(id, label, (min, max, step), get, set);
    r.kind = Kind::Slider { min, max, step };
    r
}

fn text_like(
    kind: Kind,
    id: &str,
    label: String,
    get: impl Fn(&Config) -> String + 'static,
    set: fn(&mut Config, String),
) -> Row {
    row(
        id,
        label,
        kind,
        Box::new(move |c| Value::Text(get(c))),
        Box::new(move |c, v| match v {
            Value::Text(t) => {
                set(c, t);
                Ok(())
            }
            _ => Err(Invalid),
        }),
    )
}

fn choice(
    id: &str,
    label: String,
    options: Vec<Opt>,
    get: impl Fn(&Config) -> String + 'static,
    set: fn(&mut Config, String),
) -> Row {
    text_like(Kind::Choice(options), id, label, get, set)
}

fn segmented(
    id: &str,
    label: String,
    options: Vec<Opt>,
    get: impl Fn(&Config) -> String + 'static,
    set: fn(&mut Config, String),
) -> Row {
    text_like(Kind::Segmented(options), id, label, get, set)
}

fn text(id: &str, label: String, get: fn(&Config) -> String, set: fn(&mut Config, String)) -> Row {
    text_like(Kind::Text, id, label, get, set)
}

fn folder(
    id: &str,
    label: String,
    get: fn(&Config) -> String,
    set: fn(&mut Config, String),
) -> Row {
    text_like(Kind::Folder, id, label, get, set)
}

fn when(mut row: Row, enabled: fn(&Config) -> bool) -> Row {
    row.enabled = Box::new(enabled);
    row
}

fn when_boxed(mut row: Row, enabled: impl Fn(&Config) -> bool + 'static) -> Row {
    row.enabled = Box::new(enabled);
    row
}

fn hinted(mut row: Row, hint: String) -> Row {
    row.hint = hint;
    row
}

fn header(id: &str, label: String) -> Row {
    row(
        id,
        label,
        Kind::Header,
        Box::new(|_| Value::Text(String::new())),
        Box::new(|_, _| Err(Invalid)),
    )
}

fn info(id: &str, label: String, value: String) -> Row {
    row(
        id,
        label,
        Kind::Info,
        Box::new(move |_| Value::Text(value.clone())),
        Box::new(|_, _| Err(Invalid)),
    )
}

/// The profile the video and audio rows edit: `video.profile`, created on first write.
pub(crate) trait Current {
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

/// The rows of a section (empty for the sections that have a page of their own). Rows that do
/// not apply with the current settings are left out, so a page only shows what matters.
pub fn rows(section: Section, env: &Env, config: &Config) -> Vec<Row> {
    match section {
        Section::General => pages::general(env, config),
        Section::Capture => pages::capture(env, config),
        Section::Video => pages::video(env, config),
        Section::Audio => pages::audio(env, config),
        Section::About => pages::about(env),
        Section::Gallery | Section::Shortcuts => Vec::new(),
    }
}

/// Sets every row of `section` back to its defaults. The shortcuts and profile pages reset
/// through their own modules.
pub fn reset(section: Section, env: &Env, config: &mut Config) {
    let defaults = Config::default();
    // The rows of the defaults: a row hidden now (a JPEG quality while PNG is chosen) is reset too.
    for row in rows(section, env, &defaults) {
        if matches!(row.kind, Kind::Info | Kind::Header | Kind::Profile(_)) {
            continue;
        }
        let value = row.value(&defaults);
        let _ = row.apply(config, value);
    }
    if section == Section::Video {
        // The options of the custom preset are not rows of the defaults.
        config.cur_mut().params.clear();
    }
    if section == Section::Audio {
        // The sources depend on the devices: the whole audio part goes back to the defaults.
        config.cur_mut().audio = Profile::default().audio;
    }
    if section == Section::Shortcuts {
        config.hotkeys = defaults.hotkeys;
    }
}

/// Things wrong with the current video settings, for the user to read (empty when fine).
pub fn video_problems(config: &Config, source: (u32, u32), lang: Lang) -> Vec<String> {
    use vixeeny_encode::registry::Registry;
    use vixeeny_encode::validate::{Context, Severity, validate};
    static REGISTRY: std::sync::LazyLock<Option<Registry>> =
        std::sync::LazyLock::new(|| Registry::builtin().ok());
    let Some(registry) = REGISTRY.as_ref() else {
        return Vec::new();
    };
    let ctx = Context {
        registry,
        source,
        probe: None,
    };
    validate(&config.cur(), &ctx)
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| problem_text(&i.kind, lang))
        .collect()
}

/// What a problem of the video settings means, for the user.
fn problem_text(kind: &vixeeny_encode::validate::IssueKind, lang: Lang) -> String {
    use vixeeny_encode::validate::IssueKind as I;
    let (en, fr): (String, String) = match kind {
        I::UnknownEncoder(_) | I::NotAvailable => (
            "The chosen encoder is not available on this computer.".into(),
            "L'encodeur choisi n'est pas disponible sur cet ordinateur.".into(),
        ),
        I::UnknownContainer(_) | I::ContainerNotSupported { .. } => (
            "This encoder cannot write to the chosen container: pick another one.".into(),
            "Cet encodeur ne peut pas écrire dans le conteneur choisi : choisissez-en un autre."
                .into(),
        ),
        I::UnknownChroma(_) | I::BadDepth(_) | I::FormatNotSupported { .. } => (
            "This encoder does not support the chosen colour format.".into(),
            "Cet encodeur ne gère pas le format de couleur choisi.".into(),
        ),
        I::UnknownHdrSetting(_) | I::HdrNotSupported | I::HdrNeedsHevcOrAv1 => (
            "HDR needs an HEVC or AV1 encoder that supports it.".into(),
            "Le HDR demande un encodeur HEVC ou AV1 qui le gère.".into(),
        ),
        I::HdrNeeds10Bit => (
            "HDR needs 10-bit colour.".into(),
            "Le HDR demande la couleur 10 bits.".into(),
        ),
        I::HdrContainer(_) => (
            "HDR cannot be stored in this container.".into(),
            "Le HDR ne peut pas être enregistré dans ce conteneur.".into(),
        ),
        I::VfrContainer(_) => (
            "A variable frame rate needs MKV or WebM.".into(),
            "Une cadence variable demande MKV ou WebM.".into(),
        ),
        I::AudioCodec { .. } | I::UnknownAudioCodec(_) => (
            "The audio codec does not fit the container.".into(),
            "Le codec audio ne convient pas au conteneur.".into(),
        ),
        I::UnknownPreset(_) => (
            "Unknown preset: choose one again.".into(),
            "Préréglage inconnu : choisissez-en un.".into(),
        ),
        I::ZeroFps | I::BadResolution(_) => (
            "The resolution or the frame rate is not valid.".into(),
            "La résolution ou la cadence n'est pas valide.".into(),
        ),
        I::FramerateTooHigh { max_fps } => (
            format!("Too many frames per second for this codec at this size (at most {max_fps})."),
            format!("Trop d'images par seconde pour ce codec à cette taille ({max_fps} au plus)."),
        ),
    };
    if lang == Lang::Fr { fr } else { en }
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
            let mut ids: Vec<_> = rows.iter().map(|r| r.id.clone()).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), rows.len(), "{section:?}");
            for row in rows
                .iter()
                .filter(|r| !matches!(r.kind, Kind::Info | Kind::Header))
            {
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

    fn has(section: Section, c: &Config, id: &str) -> bool {
        rows(section, &env(), c).iter().any(|r| r.id == id)
    }

    #[test]
    fn rows_that_do_not_apply_are_left_out() {
        let mut c = Config::default();
        assert!(!has(Section::Capture, &c, "jpeg_quality"));
        c.image.format = "jpeg".into();
        assert!(has(Section::Capture, &c, "jpeg_quality"));
        assert!(!has(Section::Capture, &c, "avif_depth"));
        assert!(has(Section::General, &c, "widget_corner"));
        c.recording_widget.enabled = false;
        assert!(!has(Section::General, &c, "widget_corner"));
        let about = rows(Section::About, &env(), &c);
        assert!(row(&about, "auto_update").enabled(&c));
        c.general.check_updates = false;
        assert!(!row(&about, "auto_update").enabled(&c));
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
    fn the_video_page_offers_what_the_monitors_can_show() {
        let rates = |e: &Env, c: &Config| match &row(&rows(Section::Video, e, c), "fps").kind {
            Kind::Segmented(o) => o.iter().map(|o| o.value.clone()).collect::<Vec<_>>(),
            _ => panic!("fps is a segmented row"),
        };
        let mut c = Config::default();
        let slow = env();
        assert_eq!(rates(&slow, &c), ["30", "60"]);
        let fast = env().with_display(Display {
            hdr: true,
            max_refresh: 144,
        });
        assert_eq!(rates(&fast, &c), ["30", "60", "120"]);
        // A rate from before the shorter list shows as the closest one offered.
        c.cur_mut().fps = 144;
        let fps = |e: &Env| row(&rows(Section::Video, e, &c), "fps").value(&c);
        assert_eq!(fps(&fast), Value::Text("120".into()));
        assert_eq!(fps(&slow), Value::Text("60".into()));
        // HDR: greyed out and off while Windows does not show it.
        c.cur_mut().hdr = "keep_hdr".into();
        let off = rows(Section::Video, &slow, &c);
        assert!(!row(&off, "hdr").enabled(&c));
        assert_eq!(row(&off, "hdr").value(&c), Value::Bool(false));
        let on = rows(Section::Video, &fast, &c);
        assert!(row(&on, "hdr").enabled(&c));
        assert_eq!(row(&on, "hdr").value(&c), Value::Bool(true));
        // Only fragmented MP4, which older settings read as.
        c.cur_mut().container = "mp4_hybrid".into();
        let container = row(&on, "container");
        assert_eq!(container.value(&c), Value::Text("mp4_fragmented".into()));
        assert!(matches!(&container.kind, Kind::Choice(o) if o.len() == 3));
    }

    #[test]
    fn the_split_is_a_kind_and_an_amount() {
        let mut c = Config::default();
        assert!(!has(Section::Video, &c, "split_amount"));
        let video = rows(Section::Video, &env(), &c);
        let kind = row(&video, "split");
        kind.apply(&mut c, Value::Text("size".into())).unwrap();
        assert_eq!(c.cur().split.mode, "size:2048");
        let video = rows(Section::Video, &env(), &c);
        let (kind, amount) = (row(&video, "split"), row(&video, "split_amount"));
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
        let folders = rows(Section::General, &env(), &c);
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
        let ocr = rows(Section::Capture, &env(), &c);
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
        let replay = rows(Section::Video, &env(), &c);
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
        row(&replay, "replay_storage")
            .apply(&mut c, Value::Text("disk".into()))
            .unwrap();
        assert_eq!(c.replay.storage, "disk");
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
        assert_eq!(rows(Section::General, &fr, &c)[1].label, "Langue");
        assert_eq!(rows(Section::General, &en, &c)[1].label, "Language");
        assert!(!fr.encoders.is_empty());
    }

    fn with_nvenc() -> Env {
        use vixeeny_encode::probe::{EncoderProbe, FormatProbe, ProbeResult};
        use vixeeny_encode::registry::Chroma;
        let probe = ProbeResult {
            encoders: vec![EncoderProbe {
                id: "nvenc_h264".into(),
                adapter: Some(0),
                formats: vec![FormatProbe {
                    depth: 8,
                    chroma: Chroma::C420,
                    uhd: true,
                    hdr: false,
                }],
            }],
            ..ProbeResult::default()
        };
        env().with_probe(Some(probe), false)
    }

    fn encoder_options(rows_: &[Row]) -> Vec<String> {
        match &row(rows_, "encoder").kind {
            Kind::Choice(options) => options.iter().map(|o| o.value.clone()).collect(),
            // Nothing to choose: the row says why.
            _ => Vec::new(),
        }
    }

    #[test]
    fn the_encoders_listed_are_the_ones_of_the_chosen_kind_that_exist_here() {
        let mut c = Config::default();
        // Hardware by default; nothing listed before the probe has run.
        assert!(encoder_options(&rows(Section::Video, &env(), &c)).is_empty());
        let e = with_nvenc();
        let video = rows(Section::Video, &e, &c);
        assert_eq!(encoder_options(&video), ["nvenc_h264"]);
        // `auto` shows the encoder it resolves to.
        assert_eq!(
            row(&video, "encoder").value(&c),
            Value::Text("nvenc_h264".into())
        );
        // Software lists the software encoders only, and resets the choice.
        row(&video, "encoder_kind")
            .apply(&mut c, Value::Text("software".into()))
            .unwrap();
        let video = rows(Section::Video, &e, &c);
        let software = encoder_options(&video);
        assert!(software.contains(&"libx264".to_owned()));
        assert!(!software.contains(&"nvenc_h264".to_owned()));
        assert_eq!(c.cur().encoder, "auto");
    }

    #[test]
    fn ten_bit_needs_an_encoder_that_can_do_it() {
        let mut c = Config::default();
        c.cur_mut().encoder_kind = "software".into();
        let video = rows(Section::Video, &env(), &c);
        assert!(row(&video, "ten_bit").enabled(&c));
        row(&video, "ten_bit")
            .apply(&mut c, Value::Bool(true))
            .unwrap();
        assert_eq!(c.cur().depth, 10);
        // The only probed hardware encoder does 8-bit only.
        let hw = Config::default();
        let video = rows(Section::Video, &with_nvenc(), &hw);
        assert!(!row(&video, "ten_bit").enabled(&hw));
    }

    #[test]
    fn hdr_is_one_switch() {
        let mut c = Config::default();
        let images = rows(Section::Capture, &env(), &c);
        let hdr = row(&images, "image_hdr");
        assert_eq!(hdr.value(&c), Value::Bool(false));
        assert_eq!(c.image.hdr, "tonemap_sdr");
        hdr.apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.image.hdr, "keep_hdr");
        let video = rows(Section::Video, &env(), &c);
        row(&video, "hdr").apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.cur().hdr, "keep_hdr");
        row(&video, "hdr")
            .apply(&mut c, Value::Bool(false))
            .unwrap();
        assert_eq!(c.cur().hdr, "tonemap_sdr");
    }

    #[test]
    fn presets_are_best_quality_light_or_custom_and_custom_shows_the_encoder_options() {
        let mut c = Config::default();
        c.cur_mut().encoder_kind = "software".into();
        let ids = |c: &Config| -> Vec<String> {
            rows(Section::Video, &env(), c)
                .into_iter()
                .map(|r| r.id)
                .collect()
        };
        assert!(!ids(&c).iter().any(|i| i.starts_with("p:")));
        // Older presets are shown as custom.
        c.cur_mut().preset = "balanced".into();
        let video = rows(Section::Video, &env(), &c);
        assert_eq!(
            row(&video, "preset").value(&c),
            Value::Text("custom".into())
        );
        c.cur_mut().preset = "custom".into();
        let video = rows(Section::Video, &env(), &c);
        assert!(ids(&c).iter().any(|i| i == "p:crf"));
        // A custom option is stored by its key and read back (the default until set).
        let crf = row(&video, "p:crf");
        assert_eq!(crf.value(&c), Value::Int(23));
        crf.apply(&mut c, Value::Int(18)).unwrap();
        assert_eq!(c.cur().params["crf"], "18");
        assert_eq!(crf.value(&c), Value::Int(18));
        // Resetting the page forgets them.
        reset(Section::Video, &env(), &mut c);
        assert!(c.cur().params.is_empty());
    }

    #[test]
    fn audio_sources_are_ticked_one_by_one() {
        let mut c = Config::default();
        let e = env().with_audio(AudioDevices {
            outputs: vec![AudioEntry {
                id: "{out-1}".into(),
                name: "Headset".into(),
            }],
            inputs: vec![AudioEntry {
                id: "{in-1}".into(),
                name: "Blue Yeti".into(),
            }],
            programs: vec![AudioEntry {
                id: "spotify.exe".into(),
                name: "Spotify".into(),
            }],
        });
        let audio = rows(Section::Audio, &e, &c);
        // The profile records the default output; the rest is off.
        assert_eq!(row(&audio, "src:system").value(&c), Value::Bool(true));
        assert_eq!(row(&audio, "src:out:{out-1}").value(&c), Value::Bool(false));
        row(&audio, "src:mic:{in-1}")
            .apply(&mut c, Value::Bool(true))
            .unwrap();
        row(&audio, "src:app:spotify.exe")
            .apply(&mut c, Value::Bool(true))
            .unwrap();
        row(&audio, "src:system")
            .apply(&mut c, Value::Bool(false))
            .unwrap();
        assert_eq!(c.cur().audio.sources, ["mic:{in-1}", "app:spotify.exe"]);
        // A source that is not available now stays listed, so it can be unticked.
        c.cur_mut().audio.sources.push("app:closed.exe".into());
        let audio = rows(Section::Audio, &e, &c);
        assert_eq!(
            row(&audio, "src:app:closed.exe").value(&c),
            Value::Bool(true)
        );
    }

    #[test]
    fn bad_video_settings_are_reported() {
        let mut c = Config::default();
        assert!(video_problems(&c, (1920, 1080), Lang::En).is_empty());
        c.profiles.insert("default".into(), Profile::default());
        c.profiles.get_mut("default").unwrap().container = "avi".into();
        let problems = video_problems(&c, (1920, 1080), Lang::Fr);
        assert!(
            problems.iter().any(|p| p.contains("conteneur")),
            "{problems:?}"
        );
    }
}
