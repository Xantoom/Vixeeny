// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings model (plan 5.13): every setting is a [`Row`] with a way to read it from a
//! [`Config`] and a validated way to write it. The settings window only draws rows; a change is
//! applied the moment it is made.
//!
//! No UI and no OS code here, so all of it is tested on any machine.

pub mod encoders;
pub mod machine;
mod pages;
pub mod shortcuts;

use vixeeny_common::config::{Config, Video};
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_encode::probe::ProbeResult;

pub use encoders::{EncoderInfo, param_label};
pub use machine::Machine;

/// The sections of the settings window, in the order of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    /// The side strip and the recording widget.
    Overlay,
    /// Screenshots: folder, format, capture options.
    Image,
    /// Recording: folder, picture, encoder, file.
    Video,
    Audio,
    /// The replay buffer.
    Replay,
    Shortcuts,
    Updates,
    /// Version, system, links.
    About,
}

impl Section {
    pub const ALL: [Self; 9] = [
        Self::General,
        Self::Overlay,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Replay,
        Self::Shortcuts,
        Self::Updates,
        Self::About,
    ];

    pub const fn title(self) -> Key {
        match self {
            Self::General => Key::SecGeneral,
            Self::Overlay => Key::SecOverlay,
            Self::Image => Key::SecImage,
            Self::Video => Key::SecVideo,
            Self::Audio => Key::SecAudio,
            Self::Replay => Key::SecReplay,
            Self::Shortcuts => Key::SecShortcuts,
            Self::Updates => Key::SecUpdates,
            Self::About => Key::SecAbout,
        }
    }

    /// Whether the section is made of [`Row`]s (the shortcuts and the about page have a page
    /// of their own).
    pub const fn is_rows(self) -> bool {
        !matches!(self, Self::Shortcuts | Self::About)
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
    /// The title of the options under it, not one to choose.
    pub heading: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Toggle,
    /// A drop-down list.
    Choice(Vec<Opt>),
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
    /// A folder: the path, a button to choose another one, a button to open it.
    Folder,
    /// Read-only.
    Info,
    /// Starts a new group of rows (shown apart from the one above); nothing to edit.
    Header,
    /// Shows or hides the rows under it (its value: whether they are shown). Its state is the
    /// window's, not a setting: see [`Env::open`].
    Expander,
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
    /// A picture before the label (a program's icon).
    pub icon: Option<Icon>,
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
            (Kind::Choice(options), Value::Text(t)) => {
                if !options.iter().any(|o| o.value == t && !o.heading) {
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

/// A small picture, RGBA rows with straight alpha.
#[derive(Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub rgba: std::sync::Arc<[u8]>,
}

impl std::fmt::Debug for Icon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Icon({}x{})", self.width, self.height)
    }
}

/// An output device, microphone or program that can be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEntry {
    /// The device id, or the executable name of a program.
    pub id: String,
    pub name: String,
    /// A program's icon.
    pub icon: Option<Icon>,
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
    /// What this computer is made of, once the host has read it.
    pub machine: Machine,
    /// The expanders that are open (by row id); all closed at first.
    pub open: std::collections::BTreeSet<String>,
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
            machine: Machine::default(),
            open: std::collections::BTreeSet::new(),
        }
    }

    pub fn with_open(mut self, open: std::collections::BTreeSet<String>) -> Self {
        self.open = open;
        self
    }

    pub fn with_machine(mut self, machine: Machine) -> Self {
        self.machine = machine;
        self
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

/// The title of the options that follow, in a list.
fn heading(label: impl Into<String>) -> Opt {
    Opt {
        value: String::new(),
        label: label.into(),
        heading: true,
    }
}

fn opt(value: &str, label: impl Into<String>) -> Opt {
    Opt {
        heading: false,
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
        icon: None,
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

fn hinted(mut row: Row, hint: String) -> Row {
    row.hint = hint;
    row
}

/// Starts a new group: the rows below it are shown apart.
fn header(id: &str) -> Row {
    row(
        id,
        String::new(),
        Kind::Header,
        Box::new(|_| Value::Text(String::new())),
        Box::new(|_, _| Err(Invalid)),
    )
}

/// A closed section of rows, open when `env` says so: the caller adds the rows under it only
/// then.
fn expander(env: &Env, id: &str, label: String) -> (Row, bool) {
    let open = env.open.contains(id);
    let row = row(
        id,
        label,
        Kind::Expander,
        Box::new(move |_| Value::Bool(open)),
        Box::new(|_, _| Err(Invalid)),
    );
    (row, open)
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

/// The rows of a section (empty for the sections that have a page of their own). Rows that do
/// not apply with the current settings are left out, so a page only shows what matters.
pub fn rows(section: Section, env: &Env, config: &Config) -> Vec<Row> {
    match section {
        Section::General => pages::general(env),
        Section::Overlay => pages::overlay(env, config),
        Section::Image => pages::image(env, config),
        Section::Video => pages::video(env, config),
        Section::Audio => pages::audio(env, config),
        Section::Replay => pages::replay(env, config),
        Section::Updates => pages::updates(env),
        Section::Shortcuts | Section::About => Vec::new(),
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
    // The variable frame rate of older settings is not used any more.
    let profile = Video {
        vfr: false,
        ..config.video.clone()
    };
    validate(&profile, &ctx)
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
        I::UnknownChroma(_) | I::FormatNotSupported { .. } => (
            "This encoder does not support the chosen colour format.".into(),
            "Cet encodeur ne gère pas le format de couleur choisi.".into(),
        ),
        I::UnknownHdrSetting(_) | I::HdrNotSupported | I::HdrNeedsHevcOrAv1 => (
            "HDR needs an HEVC or AV1 encoder that supports it.".into(),
            "Le HDR demande un encodeur HEVC ou AV1 qui le gère.".into(),
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
    use vixeeny_common::config::TARGET_SOURCE;

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
                .filter(|r| !matches!(r.kind, Kind::Info | Kind::Header | Kind::Expander))
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
        c.video.split.mode = "size:2048".into();
        let video = rows(Section::Video, &env(), &c);
        let amount = row(&video, "split_amount");
        amount.apply(&mut c, Value::Int(5_000_000)).unwrap();
        assert_eq!(c.video.split.mode, "size:1000000");
        amount.apply(&mut c, Value::Int(-5)).unwrap();
        assert_eq!(c.video.split.mode, "size:1");
    }

    fn has(section: Section, c: &Config, id: &str) -> bool {
        rows(section, &env(), c).iter().any(|r| r.id == id)
    }

    #[test]
    fn rows_that_do_not_apply_are_left_out() {
        let mut c = Config::default();
        assert!(!has(Section::Image, &c, "jpeg_quality"));
        c.image.format = "jpeg".into();
        assert!(has(Section::Image, &c, "jpeg_quality"));
        assert!(!has(Section::Image, &c, "avif_depth"));
        assert!(has(Section::Overlay, &c, "widget_corner"));
        c.recording_widget.enabled = false;
        assert!(!has(Section::Overlay, &c, "widget_corner"));
        let updates = rows(Section::Updates, &env(), &c);
        assert!(row(&updates, "auto_update").enabled(&c));
        c.general.check_updates = false;
        assert!(!row(&updates, "auto_update").enabled(&c));
    }

    #[test]
    fn video_rows_edit_the_video_settings() {
        let mut c = Config::default();
        let video = rows(Section::Video, &env(), &c);
        row(&video, "fps")
            .apply(&mut c, Value::Text("30".into()))
            .unwrap();
        row(&video, "container")
            .apply(&mut c, Value::Text("mkv".into()))
            .unwrap();
        assert_eq!((c.video.fps, c.video.container.as_str()), (30, "mkv"));
    }

    #[test]
    fn the_video_page_offers_what_the_monitors_can_show() {
        let rates = |e: &Env, c: &Config| match &row(&rows(Section::Video, e, c), "fps").kind {
            Kind::Choice(o) => o.iter().map(|o| o.value.clone()).collect::<Vec<_>>(),
            _ => panic!("fps is a list"),
        };
        let mut c = Config::default();
        let slow = env();
        assert_eq!(rates(&slow, &c), ["24", "30", "60"]);
        let fast = env().with_display(Display {
            hdr: true,
            max_refresh: 144,
        });
        assert_eq!(rates(&fast, &c), ["24", "30", "60", "90", "120", "144"]);
        let fps = |e: &Env, c: &Config| row(&rows(Section::Video, e, c), "fps").value(c);
        assert_eq!(fps(&slow, &c), Value::Text("60".into()));
        // A rate this machine does not offer shows as the closest one offered.
        c.video.fps = 144;
        assert_eq!(fps(&fast, &c), Value::Text("144".into()));
        assert_eq!(fps(&slow, &c), Value::Text("60".into()));
        // HDR: offered only while Windows shows it.
        c.video.hdr = "keep_hdr".into();
        assert!(!has(Section::Video, &c, "hdr"));
        assert!(!has(Section::Image, &c, "image_hdr"));
        let on = rows(Section::Video, &fast, &c);
        assert!(row(&on, "hdr").enabled(&c));
        assert_eq!(row(&on, "hdr").value(&c), Value::Bool(true));
        // Only fragmented MP4, which older settings read as.
        c.video.container = "mp4_hybrid".into();
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
        assert_eq!(c.video.split.mode, "size:2048");
        let video = rows(Section::Video, &env(), &c);
        let (kind, amount) = (row(&video, "split"), row(&video, "split_amount"));
        amount.apply(&mut c, Value::Int(4096)).unwrap();
        assert_eq!(c.video.split.mode, "size:4096");
        kind.apply(&mut c, Value::Text("duration".into())).unwrap();
        assert_eq!(c.video.split.mode, "duration:4096");
        assert_eq!(kind.value(&c), Value::Text("duration".into()));
        kind.apply(&mut c, Value::Text("off".into())).unwrap();
        assert_eq!(c.video.split.mode, "off");
    }

    #[test]
    fn every_folder_sits_on_its_own_page() {
        let mut c = Config::default();
        for (section, id) in [
            (Section::Image, "dir_images"),
            (Section::Video, "dir_videos"),
            (Section::Replay, "dir_replays"),
        ] {
            let page = rows(section, &env(), &c);
            assert!(matches!(row(&page, id).kind, Kind::Folder), "{id}");
        }
        let video = rows(Section::Video, &env(), &c);
        row(&video, "dir_videos")
            .apply(&mut c, Value::Text(" D:\\Clips ".into()))
            .unwrap();
        assert_eq!(c.paths.videos, "D:\\Clips");
        // Each kind names its files its own way; a template cannot be emptied.
        let image = rows(Section::Image, &env(), &c);
        row(&image, "template:images")
            .apply(&mut c, Value::Text("{app}-{time}".into()))
            .unwrap();
        row(&image, "foreground_app:images")
            .apply(&mut c, Value::Bool(false))
            .unwrap();
        row(&video, "template:videos")
            .apply(&mut c, Value::Text(" ".into()))
            .unwrap();
        assert_eq!(c.paths.naming.images.template, "{app}-{time}");
        assert!(!c.paths.naming.images.use_foreground_app);
        assert_eq!(c.paths.naming.videos.template, "{app}_{date}_{time}");
        assert!(c.paths.naming.videos.use_foreground_app);
        let replay = rows(Section::Replay, &env(), &c);
        assert!(replay.iter().any(|r| r.id == "template:replays"));
        let general = rows(Section::General, &env(), &c);
        assert!(!general.iter().any(|r| r.id.starts_with("template")));
    }

    #[test]
    fn each_image_format_offers_its_encoder_options() {
        let mut c = Config::default();
        let ids = |c: &Config| -> Vec<String> {
            rows(Section::Image, &env(), c)
                .into_iter()
                .map(|r| r.id)
                .collect()
        };
        for (format, own) in [
            ("png", &["png_compression", "png_optimize"][..]),
            ("jpeg", &["jpeg_quality"]),
            ("webp", &["webp_lossless", "webp_effort"]),
            ("avif", &["avif_quality", "avif_depth", "avif_speed"]),
            ("jxl", &["jxl_lossless", "jxl_effort"]),
        ] {
            c.image.format = format.into();
            let ids = ids(&c);
            for id in own {
                assert!(ids.iter().any(|i| i == id), "{format}: {id}");
            }
            // Only its own.
            let others = ids
                .iter()
                .filter(|i| i.contains('_') && !i.starts_with(format))
                .filter(|i| {
                    ["png", "jpeg", "webp", "avif", "jxl"]
                        .iter()
                        .any(|f| i.starts_with(f))
                })
                .count();
            assert_eq!(others, 0, "{format}");
        }
        // A lossy quality only when it is not lossless.
        c.image.format = "webp".into();
        assert!(!ids(&c).iter().any(|i| i == "webp_quality"));
        c.image.webp.lossless = false;
        assert!(ids(&c).iter().any(|i| i == "webp_quality"));
    }

    #[test]
    fn the_replay_keeps_its_duration_and_storage() {
        let mut c = Config::default();
        let replay = rows(Section::Replay, &env(), &c);
        let duration = row(&replay, "replay_duration");
        duration.apply(&mut c, Value::Text("1800".into())).unwrap();
        assert_eq!(c.replay.duration_seconds, 1800);
        // An older duration shows as the closest one offered.
        c.replay.duration_seconds = 45;
        assert_eq!(duration.value(&c), Value::Text("30".into()));
        row(&replay, "replay_storage")
            .apply(&mut c, Value::Text("disk".into()))
            .unwrap();
        assert_eq!(c.replay.storage, "disk");
    }

    #[test]
    fn labels_follow_the_language() {
        let c = Config::default();
        let fr = Env::new(Lang::Fr, "1");
        let en = env();
        assert_eq!(rows(Section::General, &fr, &c)[0].label, "Langue");
        assert_eq!(rows(Section::General, &en, &c)[0].label, "Language");
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
        assert_eq!(c.video.encoder, "auto");
    }

    #[test]
    fn the_video_page_speaks_in_pixels_and_offers_16_9_on_a_wide_screen() {
        use crate::machine::{Machine, Screen};
        let screen = |width, height| Machine {
            screens: vec![Screen {
                number: 1,
                primary: true,
                name: "Screen".into(),
                width,
                height,
                hz: 144,
            }],
            ..Machine::default()
        };
        let options = |rows_: &[Row], id: &str| match &row(rows_, id).kind {
            Kind::Choice(o) => o.iter().map(|o| o.label.clone()).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let mut c = Config::default();
        let uhd = env().with_machine(screen(3840, 2160));
        let video = rows(Section::Video, &uhd, &c);
        assert_eq!(
            options(&video, "resolution"),
            [
                "The screen's (3840 × 2160)",
                "2560 × 1440 (QHD)",
                "1920 × 1080 (Full HD)",
                "1280 × 720 (HD)",
                "854 × 480"
            ]
        );
        // 16:9: nothing to choose.
        assert!(!video.iter().any(|r| r.id == "aspect"));
        assert!(!video.iter().any(|r| r.id == "ten_bit"));
        // An ultrawide screen can be recorded as 16:9, and the sizes follow.
        let wide = env().with_machine(screen(3440, 1440));
        let video = rows(Section::Video, &wide, &c);
        assert_eq!(
            options(&video, "aspect"),
            ["The screen's (21:9)", "16:9, cropped in the middle"]
        );
        assert_eq!(options(&video, "resolution")[1], "2580 × 1080");
        row(&video, "aspect")
            .apply(&mut c, Value::Text("16:9".into()))
            .unwrap();
        let video = rows(Section::Video, &wide, &c);
        assert_eq!(
            options(&video, "resolution")[..2],
            ["The screen's (2560 × 1440)", "1920 × 1080 (Full HD)"]
        );
    }

    #[test]
    fn expert_settings_stay_folded_until_opened() {
        let mut c = Config::default();
        c.video.preset = "custom".into();
        let closed = rows(Section::Video, &with_nvenc(), &c);
        assert!(matches!(row(&closed, "expert_video").kind, Kind::Expander));
        assert!(closed.iter().any(|r| r.id == "p:preset"));
        assert!(
            !closed
                .iter()
                .any(|r| r.id == "p:multipass" || r.id == "chroma")
        );
        let open = with_nvenc().with_open(["expert_video".to_owned()].into());
        let opened = rows(Section::Video, &open, &c);
        assert!(opened.iter().any(|r| r.id == "p:multipass"));
        assert!(opened.iter().any(|r| r.id == "chroma"));
    }

    #[test]
    fn hdr_is_one_switch() {
        let mut c = Config::default();
        let env = env().with_display(Display {
            hdr: true,
            max_refresh: 60,
        });
        let images = rows(Section::Image, &env, &c);
        let hdr = row(&images, "image_hdr");
        assert_eq!(hdr.value(&c), Value::Bool(false));
        assert_eq!(c.image.hdr, "tonemap_sdr");
        hdr.apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.image.hdr, "keep_hdr");
        let video = rows(Section::Video, &env, &c);
        row(&video, "hdr").apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.video.hdr, "keep_hdr");
        row(&video, "hdr")
            .apply(&mut c, Value::Bool(false))
            .unwrap();
        assert_eq!(c.video.hdr, "tonemap_sdr");
    }

    #[test]
    fn presets_are_best_quality_light_or_custom_and_custom_shows_the_encoder_options() {
        let mut c = Config::default();
        c.video.encoder_kind = "software".into();
        let ids = |c: &Config| -> Vec<String> {
            rows(Section::Video, &env(), c)
                .into_iter()
                .map(|r| r.id)
                .collect()
        };
        assert!(!ids(&c).iter().any(|i| i.starts_with("p:")));
        // Older presets are shown as custom.
        c.video.preset = "balanced".into();
        let video = rows(Section::Video, &env(), &c);
        assert_eq!(
            row(&video, "preset").value(&c),
            Value::Text("custom".into())
        );
        c.video.preset = "custom".into();
        let video = rows(Section::Video, &env(), &c);
        assert!(ids(&c).iter().any(|i| i == "p:rc.quality"));
        // A custom option is stored by its key and read back (the default until set).
        let crf = row(&video, "p:rc.quality");
        assert_eq!(crf.value(&c), Value::Int(23));
        crf.apply(&mut c, Value::Int(18)).unwrap();
        assert_eq!(c.video.params["rc.quality"], "18");
        assert_eq!(crf.value(&c), Value::Int(18));
        // Values are shown by name.
        let Kind::Choice(presets) = &row(&video, "p:preset").kind else {
            panic!("the preset is a list")
        };
        assert_eq!(presets[0].label, "Ultra fast");
        // The fields follow the rate mode: a bitrate replaces the quality.
        row(&video, "p:rc.mode")
            .apply(&mut c, Value::Text("vbr".into()))
            .unwrap();
        let after = ids(&c);
        assert!(!after.iter().any(|i| i == "p:rc.quality"));
        assert!(after.iter().any(|i| i == "p:rc.bitrate"));
        assert!(after.iter().any(|i| i == "p:rc.maxrate"));
    }

    #[test]
    fn the_sound_to_record_is_one_choice_and_the_microphone_another() {
        let mut c = Config::default();
        let e = env().with_audio(AudioDevices {
            outputs: vec![AudioEntry {
                id: "{out-1}".into(),
                name: "Headset".into(),
                icon: None,
            }],
            inputs: vec![AudioEntry {
                id: "{in-1}".into(),
                name: "Blue Yeti".into(),
                icon: None,
            }],
            programs: vec![AudioEntry {
                id: "spotify.exe".into(),
                name: "Spotify".into(),
                icon: None,
            }],
        });
        let audio = rows(Section::Audio, &e, &c);
        // All the PC sound; the microphone is off, its choice greyed out.
        let capture = |c: &Config| row(&rows(Section::Audio, &e, c), "audio_capture").value(c);
        assert_eq!(capture(&c), Value::Text("system".into()));
        assert_eq!(row(&audio, "mic_on").value(&c), Value::Bool(false));
        assert!(!row(&audio, "mic_device").enabled(&c));
        // On: the Windows default, then the one picked.
        row(&audio, "mic_on")
            .apply(&mut c, Value::Bool(true))
            .unwrap();
        assert_eq!(c.video.audio.sources, ["system", "mic"]);
        match &row(&audio, "mic_device").kind {
            Kind::Choice(o) => assert_eq!(
                o.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(),
                ["Windows default", "Blue Yeti"]
            ),
            _ => panic!("mic_device is a choice"),
        }
        row(&audio, "mic_device")
            .apply(&mut c, Value::Text("mic:{in-1}".into()))
            .unwrap();
        // The recorded program: the microphone stays.
        let set = |c: &mut Config, mode: &str| {
            row(&rows(Section::Audio, &e, c), "audio_capture")
                .apply(c, Value::Text(mode.into()))
                .unwrap();
        };
        set(&mut c, "target");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", TARGET_SOURCE]);
        // One output: the first device, then the one picked.
        set(&mut c, "output");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", "out:{out-1}"]);
        let audio = rows(Section::Audio, &e, &c);
        assert_eq!(
            row(&audio, "audio_output").value(&c),
            Value::Text("out:{out-1}".into())
        );
        // Chosen programs: ticked one by one; none ticked is still that choice.
        set(&mut c, "programs");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}"]);
        assert_eq!(capture(&c), Value::Text("programs".into()));
        let audio = rows(Section::Audio, &e, &c);
        assert_eq!(row(&audio, "src:app:spotify.exe").label, "Spotify");
        row(&audio, "src:app:spotify.exe")
            .apply(&mut c, Value::Bool(true))
            .unwrap();
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", "app:spotify.exe"]);
        // A ticked program that is closed now stays listed.
        c.video.audio.sources.push("app:game.exe".into());
        let audio = rows(Section::Audio, &e, &c);
        assert_eq!(row(&audio, "src:app:game.exe").label, "game.exe (closed)");
        // Off removes the microphone, whichever it was.
        row(&audio, "mic_on")
            .apply(&mut c, Value::Bool(false))
            .unwrap();
        assert_eq!(c.video.audio.sources, ["app:spotify.exe", "app:game.exe"]);
        // Older settings without the field: read from their sources.
        let mut old = Config::default();
        old.video.audio.sources = vec!["mic".into()];
        old.video.audio.capture = "system".into();
        assert_eq!(capture(&old), Value::Text("none".into()));
    }

    #[test]
    fn bad_video_settings_are_reported() {
        let mut c = Config::default();
        assert!(video_problems(&c, (1920, 1080), Lang::En).is_empty());
        c.video.container = "avi".into();
        let problems = video_problems(&c, (1920, 1080), Lang::Fr);
        assert!(
            problems.iter().any(|p| p.contains("conteneur")),
            "{problems:?}"
        );
    }

    #[test]
    fn the_replay_says_how_big_it_will_be_and_where_auto_keeps_it() {
        use crate::machine::{Machine, Screen};
        let with_ram = |gb: u64| {
            with_nvenc().with_machine(Machine {
                ram_bytes: gb << 30,
                screens: vec![Screen {
                    number: 1,
                    primary: true,
                    name: "4K".into(),
                    width: 3840,
                    height: 2160,
                    hz: 144,
                }],
                ..Machine::default()
            })
        };
        let mut c = Config::default();
        c.replay.duration_seconds = 300;
        let hint = |env: &Env, c: &Config| {
            row(&rows(Section::Replay, env, c), "replay_storage")
                .hint
                .clone()
        };
        assert!(
            hint(&with_ram(64), &c).contains("RAM"),
            "{}",
            hint(&with_ram(64), &c)
        );
        assert!(
            hint(&with_ram(8), &c).contains("disk"),
            "{}",
            hint(&with_ram(8), &c)
        );
        c.replay.storage = "disk".into();
        assert!(!hint(&with_ram(64), &c).contains("RAM"));
        assert!(hint(&with_ram(64), &c).contains("GB"));
    }
}
