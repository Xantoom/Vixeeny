// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings model: every setting is a [`Row`] with a way to read it from a [`Config`] and a validated way to write
//! it. The settings window only draws rows; a change is applied the moment it is made.
//!
//! No UI and no OS code here, so all of it is tested on any machine.

pub mod encoders;
mod info;
pub mod machine;
mod pages;
pub mod shortcuts;

use vixeeny_common::config::{Config, Video};
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_encode::probe::ProbeResult;

pub use encoders::{EncoderInfo, param_label};
pub use machine::Machine;
pub use pages::REPLAY_PREFIX;

/// The sections of the settings window, in the order of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    /// The side strip and the recording widget.
    Overlay,
    /// Screenshots: folder, format, capture options.
    Image,
    /// Recordings: folder, picture, encoder, file, sound.
    Recording,
    /// The replay buffer.
    Replay,
    Shortcuts,
    /// Which notifications are shown.
    Notifications,
    Updates,
    /// Version, system, links.
    About,
}

impl Section {
    pub const ALL: [Self; 9] = [
        Self::General,
        Self::Overlay,
        Self::Image,
        Self::Recording,
        Self::Replay,
        Self::Shortcuts,
        Self::Notifications,
        Self::Updates,
        Self::About,
    ];

    pub const fn title(self) -> Key {
        match self {
            Self::General => Key::SecGeneral,
            Self::Overlay => Key::SecOverlay,
            Self::Image => Key::SecImage,
            Self::Recording => Key::SecRecording,
            Self::Replay => Key::SecReplay,
            Self::Shortcuts => Key::SecShortcuts,
            Self::Notifications => Key::SecNotifications,
            Self::Updates => Key::SecUpdates,
            Self::About => Key::SecAbout,
        }
    }

    /// Whether the section is made of [`Row`]s (the shortcuts and the about page have a page of their own).
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
    /// The title of the options under it, not one to choose (without a label: a line between two groups).
    pub heading: bool,
    /// The one advised: marked so in the list.
    pub recommended: bool,
    /// One that usually does harm (a tune for measurements): shown in the warning colour.
    pub caution: bool,
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
    /// A button (its value: its text) that the host answers; nothing to edit.
    Button,
    /// Starts a new group of rows (shown apart from the one above), titled by its label when it has one; nothing to
    /// edit.
    Header,
    /// Shows or hides the rows under it (its value: whether they are shown). Its state is the window's, not a setting:
    /// see [`Env::open`].
    Expander,
    /// A colour (`#rrggbb`), or the two ends of a gradient (`#rrggbb,#rrggbb`), with colours and gradients ready to
    /// take.
    Colour {
        swatches: Vec<[u8; 3]>,
        pairs: Vec<([u8; 3], [u8; 3])>,
    },
}

/// The value was refused (not one of the choices, not a number, empty where it must not be).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

type Get = Box<dyn Fn(&Config) -> Value>;
type Set = Box<dyn Fn(&mut Config, Value) -> Result<(), Invalid>>;

/// A choice drawn as a small screen to click on rather than a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Picker {
    #[default]
    List,
    /// Left, right, top, bottom: an edge of the screen.
    Edge,
    /// The top and bottom corners and middles of the screen, then a place of the user's (its value `custom:<x>:<y>`,
    /// thousandths of the room across and down).
    Corner,
}

pub struct Row {
    pub id: String,
    pub label: String,
    /// Something wrong with this setting as it is, behind a warning sign beside the label (empty = none).
    pub warning: String,
    /// What the setting does and what to choose, on hover over the label (empty = none).
    pub info: String,
    /// A picture before the label (a program's icon).
    pub icon: Option<Icon>,
    pub kind: Kind,
    /// How a choice is drawn.
    pub picker: Picker,
    /// Shown greyed out: it does not apply with the other settings as they are (the label's tooltip says why).
    pub greyed: bool,
    /// What a number counts (`kbit/s`), written after it.
    pub unit: String,
    /// Read-only text at the right of a text field (the file extension).
    pub suffix: String,
    /// How 0 reads (`Auto`), when it means something else than none.
    pub zero: String,
    /// A short line under the label (an estimate).
    pub detail: String,
    /// Belongs to the row above it (a program of its list): set in under it.
    pub nested: bool,
    /// The microphone's level, drawn live under the slider.
    pub meter: bool,
    /// A value marked on a slider's track, which it sticks to (100 % of a volume).
    pub mark: Option<i64>,
    get: Get,
    set: Set,
}

impl Row {
    pub fn value(&self, config: &Config) -> Value {
        (self.get)(config)
    }

    /// Whether it can be changed.
    pub fn enabled(&self) -> bool {
        !self.greyed && !matches!(self.kind, Kind::Info | Kind::Header)
    }

    /// Checks `value` against the kind of the row, then writes it. Numbers are brought into range and onto the step.
    pub fn apply(&self, config: &mut Config, value: Value) -> Result<(), Invalid> {
        if self.greyed {
            return Err(Invalid);
        }
        let value = match (&self.kind, value) {
            (Kind::Toggle, v @ Value::Bool(_)) => v,
            (Kind::Choice(options), Value::Text(t)) => {
                // A place of the corner picker carries where it is: `custom:<x>:<y>`.
                let chosen = match t.split_once(':') {
                    Some((value, _)) if self.picker == Picker::Corner => value,
                    _ => t.as_str(),
                };
                if !options.iter().any(|o| o.value == chosen && !o.heading) {
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
            (Kind::Colour { .. }, Value::Text(t)) => {
                let colours = t
                    .split(',')
                    .map(|c| vixeeny_common::config::parse_hex(c).ok_or(Invalid))
                    .collect::<Result<Vec<_>, _>>()?;
                if !(1..=2).contains(&colours.len()) {
                    return Err(Invalid);
                }
                let codes: Vec<String> = colours.into_iter().map(vixeeny_common::config::hex).collect();
                Value::Text(codes.join(","))
            }
            _ => return Err(Invalid),
        };
        (self.set)(config, value)
    }

    /// A number as the user reads it: grouped by thousands, with its unit (`10 000 kbit/s`).
    pub fn number_text(&self, n: i64, lang: Lang) -> String {
        if n == 0 && !self.zero.is_empty() {
            return self.zero.clone();
        }
        let digits = n.unsigned_abs().to_string();
        let sep = if lang == Lang::Fr { '\u{a0}' } else { ',' };
        let mut grouped = String::new();
        for (i, c) in digits.chars().enumerate() {
            // Four digits stay together (2048), as both languages write them.
            if i > 0 && digits.len() > 4 && (digits.len() - i).is_multiple_of(3) {
                grouped.push(sep);
            }
            grouped.push(c);
        }
        if n < 0 {
            grouped.insert(0, '-');
        }
        match self.unit.as_str() {
            "" => grouped,
            "%" if lang == Lang::En => format!("{grouped}%"),
            unit => format!("{grouped}\u{a0}{unit}"),
        }
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
    /// The programs (executable names, lower case) that look like games: those the replay lists.
    pub games: Vec<String>,
}

/// What the rows need to know about this machine.
#[derive(Debug, Clone)]
pub struct Env {
    pub lang: Lang,
    /// The language `auto` stands for (the system's).
    pub auto_lang: Lang,
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
    /// Windows' accent colour (what the `accent` colour stands for).
    pub accent: [u8; 3],
    pub admin: Admin,
}

/// Where Vixeeny is with the administrator's rights (to keep the input of the programs that have them, games often).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Admin {
    /// Nothing to offer: a portable Vixeeny, a standard account.
    #[default]
    Hidden,
    Granted,
    /// The rights can be given; `moves`: Vixeeny is installed in Program Files first (it is in the user's own folders).
    Missing {
        moves: bool,
    },
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
        Self { hdr: false, max_refresh: 60 }
    }
}

impl Env {
    pub fn new(lang: Lang, version: &str) -> Self {
        Self {
            lang,
            auto_lang: lang,
            encoders: encoders::all(),
            probe: None,
            detecting: false,
            audio: AudioDevices::default(),
            version: version.to_owned(),
            display: Display::default(),
            machine: Machine::default(),
            open: std::collections::BTreeSet::new(),
            accent: [0x00, 0x78, 0xd4],
            admin: Admin::Hidden,
        }
    }

    pub const fn with_admin(mut self, admin: Admin) -> Self {
        self.admin = admin;
        self
    }

    pub const fn with_accent(mut self, accent: [u8; 3]) -> Self {
        self.accent = accent;
        self
    }

    pub const fn with_auto_lang(mut self, lang: Lang) -> Self {
        self.auto_lang = lang;
        self
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
    Opt { value: String::new(), label: label.into(), heading: true, recommended: false, caution: false }
}

/// A line between two groups of options, in a list.
fn separator() -> Opt {
    heading(String::new())
}

fn opt(value: &str, label: impl Into<String>) -> Opt {
    Opt { heading: false, value: value.to_owned(), label: label.into(), recommended: false, caution: false }
}

/// An option that usually does harm, shown in the warning colour.
fn discouraged(value: &str, label: impl Into<String>) -> Opt {
    Opt { caution: true, ..opt(value, label) }
}

/// The option advised, marked so in the list.
fn advised(value: &str, label: impl Into<String>) -> Opt {
    Opt { recommended: true, ..opt(value, label) }
}

fn row(id: impl Into<String>, label: String, kind: Kind, get: Get, set: Set) -> Row {
    Row {
        id: id.into(),
        label,
        warning: String::new(),
        info: String::new(),
        icon: None,
        kind,
        picker: Picker::List,
        greyed: false,
        unit: String::new(),
        suffix: String::new(),
        zero: String::new(),
        detail: String::new(),
        nested: false,
        meter: false,
        mark: None,
        get,
        set,
    }
}

fn toggle(
    id: &str,
    label: String,
    get: impl Fn(&Config) -> bool + 'static,
    set: impl Fn(&mut Config, bool) + 'static,
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
    get: impl Fn(&Config) -> i64 + 'static,
    set: impl Fn(&mut Config, i64) + 'static,
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
    get: impl Fn(&Config) -> i64 + 'static,
    set: impl Fn(&mut Config, i64) + 'static,
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
    set: impl Fn(&mut Config, String) + 'static,
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
    set: impl Fn(&mut Config, String) + 'static,
) -> Row {
    text_like(Kind::Choice(options), id, label, get, set)
}

fn folder(id: &str, label: String, get: fn(&Config) -> String, set: fn(&mut Config, String)) -> Row {
    text_like(Kind::Folder, id, label, get, set)
}

/// A choice drawn as a small screen.
fn picked(picker: Picker, mut row: Row) -> Row {
    row.picker = picker;
    row
}

/// Writes `unit` after the row's number.
fn counted(mut row: Row, unit: impl Into<String>) -> Row {
    row.unit = unit.into();
    row
}

/// Sets the row in under the one above it.
fn nested(mut row: Row) -> Row {
    row.nested = true;
    row
}

/// Adds `more` to what the "i" of the row says (after the explanation of the setting).
fn hinted(mut row: Row, more: String) -> Row {
    row.info = more;
    row
}

/// Greys the row out: it does not apply as the other settings are.
fn greyed(mut row: Row) -> Row {
    row.greyed = true;
    row
}

/// Puts a warning sign beside the row, `text` on hover.
fn warned(mut row: Row, text: String) -> Row {
    row.warning = text;
    row
}

/// Starts a new group: the rows below it are shown apart, under `title` (empty: none).
fn header(id: &str, title: String) -> Row {
    row(id, title, Kind::Header, Box::new(|_| Value::Text(String::new())), Box::new(|_, _| Err(Invalid)))
}

/// A closed section of rows, open when `env` says so: the caller adds the rows under it only then.
fn expander(env: &Env, id: &str, label: String) -> (Row, bool) {
    let open = env.open.contains(id);
    let row = row(id, label, Kind::Expander, Box::new(move |_| Value::Bool(open)), Box::new(|_, _| Err(Invalid)));
    (row, open)
}

/// A button showing `text`: the host is told when it is pressed (by the row's id).
fn button(id: &str, label: String, text: String) -> Row {
    row(id, label, Kind::Button, Box::new(move |_| Value::Text(text.clone())), Box::new(|_, _| Err(Invalid)))
}

fn info(id: &str, label: String, value: String) -> Row {
    row(id, label, Kind::Info, Box::new(move |_| Value::Text(value.clone())), Box::new(|_, _| Err(Invalid)))
}

/// The rows of a section (empty for the sections that have a page of their own). Rows that do not apply with the
/// current settings are left out, so a page only shows what matters.
pub fn rows(section: Section, env: &Env, config: &Config) -> Vec<Row> {
    let mut rows = match section {
        Section::General => pages::general(env),
        Section::Overlay => pages::overlay(env, config),
        Section::Image => pages::image(env, config),
        Section::Recording => pages::recording(env, config),
        Section::Replay => pages::replay(env, config),
        Section::Notifications => pages::notifications(env, config),
        Section::Updates => pages::updates(env, config),
        Section::Shortcuts | Section::About => Vec::new(),
    };
    // What the setting does, then what the page added for it now (an estimate).
    for row in &mut rows {
        if let Some(text) = info::info(&row.id, env, config) {
            row.info = if row.info.is_empty() { text } else { format!("{text}\n\n{}", row.info) };
        }
    }
    rows
}

/// Puts the settings of a page back as they come (the about page has none).
pub fn reset(section: Section, config: &mut Config) {
    let new = Config::default();
    match section {
        Section::General => {
            let g = &mut config.general;
            g.language = new.general.language;
            g.theme = new.general.theme;
            g.autostart = new.general.autostart;
        }
        Section::Overlay => {
            config.overlay = new.overlay;
            config.recording_widget = new.recording_widget;
        }
        Section::Image => {
            config.image = new.image;
            config.paths.images = new.paths.images;
            config.paths.naming.images = new.paths.naming.images;
            config.paths.per_app_subfolder.images = new.paths.per_app_subfolder.images;
        }
        Section::Recording => {
            // What other sound modes had chosen stays at hand.
            let aside = std::mem::take(&mut config.video.audio.set_aside);
            config.video = new.video;
            config.video.audio.set_aside = aside;
            config.paths.videos = new.paths.videos;
            config.paths.naming.videos = new.paths.naming.videos;
            config.paths.per_app_subfolder.videos = new.paths.per_app_subfolder.videos;
            config.disk_alert.recordings = new.disk_alert.recordings;
        }
        Section::Replay => {
            config.replay = new.replay;
            config.paths.replays = new.paths.replays;
            config.paths.naming.replays = new.paths.naming.replays;
            config.paths.per_app_subfolder.replays = new.paths.per_app_subfolder.replays;
            config.disk_alert.replays = new.disk_alert.replays;
        }
        Section::Shortcuts => config.hotkeys = new.hotkeys,
        Section::Notifications => config.notifications = new.notifications,
        Section::Updates => {
            config.general.auto_update = new.general.auto_update;
            config.general.check_updates = new.general.check_updates;
            config.general.whats_new = new.general.whats_new;
        }
        Section::About => {}
    }
}

/// Whether a page holds a setting changed from how it comes.
pub fn differs(section: Section, config: &Config) -> bool {
    let mut fresh = config.clone();
    reset(section, &mut fresh);
    fresh != *config
}

/// All the settings as they come.
pub fn reset_all(config: &mut Config) {
    *config = Config::default();
}

/// A setting that answers a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub section: Section,
    /// The row to bring into view.
    pub id: String,
    pub label: String,
}

/// Lower case without accents, for comparing what is typed with what is written.
pub fn folded(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'ÿ' => 'y',
            '’' => '\'',
            other => other,
        })
        .collect()
}

/// Whether every word of `query` begins a word of `text` (both folded), in any order.
pub fn matches(query: &str, text: &str) -> bool {
    let text = folded(text);
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let query = folded(query);
    let mut wanted = query.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).peekable();
    wanted.peek().is_some() && wanted.all(|q| words.iter().any(|w| w.starts_with(q)))
}

/// The settings whose name answers `query`, page by page, the folded ones included.
pub fn search(query: &str, env: &Env, config: &Config) -> Vec<Found> {
    let mut env = env.clone();
    for kind in vixeeny_common::config::ImageKind::ALL {
        env.open.insert(format!("image:{}:encoding", kind.name()));
    }
    env.open.insert("expert_video".into());
    env.open.insert(format!("{}expert_video", pages::REPLAY_PREFIX));
    Section::ALL
        .into_iter()
        .flat_map(|section| {
            rows(section, &env, config)
                .into_iter()
                .filter(|r| !matches!(r.kind, Kind::Header | Kind::Expander))
                .filter(|r| matches(query, &r.label))
                .map(move |r| Found { section, id: r.id, label: r.label })
        })
        .collect()
}

/// Things wrong with the replay's own video settings, for the user to read (empty when fine or when it records with the
/// videos').
pub fn replay_problems(config: &Config, source: (u32, u32), lang: Lang) -> Vec<String> {
    if !config.replay.enabled || config.replay.same_as_video {
        return Vec::new();
    }
    video_problems(&pages::replay_view(config), source, lang)
}

/// Things wrong with the current video settings, for the user to read (empty when fine).
pub fn video_problems(config: &Config, source: (u32, u32), lang: Lang) -> Vec<String> {
    use vixeeny_encode::registry::Registry;
    use vixeeny_encode::validate::{Context, Severity, validate};
    static REGISTRY: std::sync::LazyLock<Option<Registry>> = std::sync::LazyLock::new(|| Registry::builtin().ok());
    let Some(registry) = REGISTRY.as_ref() else {
        return Vec::new();
    };
    let ctx = Context { registry, source, probe: None };
    // The variable frame rate of older settings is not used any more.
    let profile = Video { vfr: false, ..config.video.clone() };
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
            "Cet encodeur ne peut pas écrire dans le conteneur choisi : choisissez-en un autre.".into(),
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
        I::VfrContainer(_) => {
            ("A variable frame rate needs MKV or WebM.".into(), "Une cadence variable demande MKV ou WebM.".into())
        }
        I::AudioCodec { .. } | I::UnknownAudioCodec(_) => (
            "The audio codec does not fit the container.".into(),
            "Le codec audio ne convient pas au conteneur.".into(),
        ),
        I::UnknownPreset(_) => {
            ("Unknown preset: choose one again.".into(), "Préréglage inconnu : choisissez-en un.".into())
        }
        I::ZeroFps | I::BadResolution(_) => (
            "The resolution or the frame rate is not valid.".into(),
            "La résolution ou la cadence n'est pas valide.".into(),
        ),
        I::TooLarge { max: (w, h) } => (
            format!("This encoder takes pictures of {w} × {h} at most: choose HEVC or AV1, or a lower resolution."),
            format!(
                "Cet encodeur accepte des images de {w} × {h} au plus : choisissez HEVC ou AV1, ou une définition plus basse."
            ),
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
        Env::new(Lang::En, "1.2.3").with_open(
            vixeeny_common::config::ImageKind::ALL
                .into_iter()
                .map(|kind| format!("image:{}:encoding", kind.name()))
                .collect(),
        )
    }

    fn row<'a>(rows: &'a [Row], id: &str) -> &'a Row {
        rows.iter().find(|r| r.id == id).unwrap()
    }

    #[test]
    fn the_rights_are_offered_by_a_button_until_given() {
        let config = Config::default();
        let general = |admin| rows(Section::General, &env().with_admin(admin), &config);
        assert!(general(Admin::Hidden).iter().all(|r| r.id != "admin"));
        for moves in [false, true] {
            let page = general(Admin::Missing { moves });
            let admin = row(&page, "admin");
            assert!(matches!(admin.kind, Kind::Button));
            assert_eq!(admin.value(&config), Value::Text("Grant".into()));
            assert!(admin.info.contains("all applications"));
        }
        let page = general(Admin::Granted);
        assert!(matches!(row(&page, "admin").kind, Kind::Button));
        assert!(!row(&page, "admin").enabled());
        assert_eq!(row(&page, "admin").value(&config), Value::Text("Granted".into()));
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
            // A greyed row takes nothing (it may show what applies instead of the setting).
            for row in rows
                .iter()
                .filter(|r| !r.greyed && !matches!(r.kind, Kind::Info | Kind::Header | Kind::Expander | Kind::Button))
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
        assert_eq!(theme.apply(&mut c, Value::Text("purple".into())), Err(Invalid));
        assert_eq!(theme.apply(&mut c, Value::Bool(true)), Err(Invalid));
        assert_eq!(c.general.theme, "dark");
        // Numbers are clamped and put on the step.
        c.video.split.mode = "size:2048".into();
        let video = rows(Section::Recording, &env(), &c);
        let amount = row(&video, "split_amount");
        amount.apply(&mut c, Value::Int(5_000_000)).unwrap();
        assert_eq!(c.video.split.mode, "size:1000000");
        amount.apply(&mut c, Value::Int(-5)).unwrap();
        assert_eq!(c.video.split.mode, "size:1");
    }

    fn has_in(rows: &[Row], id: &str) -> bool {
        rows.iter().any(|r| r.id == id)
    }

    fn has(section: Section, c: &Config, id: &str) -> bool {
        rows(section, &env(), c).iter().any(|r| r.id == id)
    }

    #[test]
    fn the_widget_goes_to_a_preset_or_a_place_of_the_user_s() {
        let mut c = Config::default();
        let place = rows(Section::Overlay, &env(), &c).into_iter().find(|r| r.id == "widget_corner").unwrap();
        place.apply(&mut c, Value::Text("bottom_center".into())).unwrap();
        assert_eq!(c.recording_widget.corner, "bottom_center");
        // A click in the picker: a place, read back as it was given.
        place.apply(&mut c, Value::Text("custom:250:1000".into())).unwrap();
        assert_eq!(c.recording_widget.corner, "custom");
        assert_eq!(c.recording_widget.custom, (0.25, 1.0));
        assert_eq!(place.value(&c), Value::Text("custom:250:1000".into()));
        // Back to a preset, then to the place kept.
        place.apply(&mut c, Value::Text("top_left".into())).unwrap();
        place.apply(&mut c, Value::Text("custom".into())).unwrap();
        assert_eq!(c.recording_widget.custom, (0.25, 1.0));
        assert!(place.apply(&mut c, Value::Text("middle:1:2".into())).is_err());
    }

    #[test]
    fn rows_that_do_not_apply_are_left_out() {
        let mut c = Config::default();
        assert!(has(Section::Image, &c, "image:defaults:jpeg_quality"));
        c.image.defaults.format = "png".into();
        assert!(!has(Section::Image, &c, "image:defaults:jpeg_quality"));
        assert!(!has(Section::Image, &c, "image:defaults:avif_depth"));
        assert!(has(Section::Overlay, &c, "widget_corner"));
        c.recording_widget.enabled = false;
        assert!(!has(Section::Overlay, &c, "widget_corner"));
    }

    #[test]
    fn one_switch_checks_and_installs_updates() {
        let mut c = Config::default();
        let updates = rows(Section::Updates, &env(), &c);
        assert_eq!(updates.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["auto_update", "whats_new"]);
        assert!(matches!(row(&updates, "auto_update").value(&c), Value::Bool(true)));
        row(&updates, "auto_update").apply(&mut c, Value::Bool(false)).unwrap();
        assert!(!c.general.check_updates && !c.general.auto_update);
        assert_eq!(row(&updates, "whats_new").value(&c), Value::Bool(true));
    }

    #[test]
    fn the_notifications_have_one_switch_that_keeps_the_others() {
        let mut c = Config::default();
        let page = rows(Section::Notifications, &env(), &c);
        row(&page, "notify_image").apply(&mut c, Value::Bool(false)).unwrap();
        row(&page, "notify_game").apply(&mut c, Value::Text("on".into())).unwrap();
        // Off: only the switch, and what was set stays for when it is on again.
        row(&page, "notifications").apply(&mut c, Value::Bool(false)).unwrap();
        let off = rows(Section::Notifications, &env(), &c);
        assert_eq!(off.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["notifications"]);
        assert!(!c.notifications.image_saved && c.notifications.game_start == "on");
        assert!(!c.notifications.game_start(true));
        row(&off, "notifications").apply(&mut c, Value::Bool(true)).unwrap();
        assert!(!c.notifications.image_saved && c.notifications.game_start(true));
        assert!(differs(Section::Notifications, &c) && !differs(Section::General, &c));
    }

    #[test]
    fn the_video_page_offers_what_the_monitors_can_show() {
        let rates = |e: &Env, c: &Config| match &row(&rows(Section::Recording, e, c), "fps").kind {
            Kind::Choice(o) => o.iter().map(|o| o.value.clone()).collect::<Vec<_>>(),
            _ => panic!("fps is a list"),
        };
        let mut c = Config::default();
        let slow = env();
        assert_eq!(rates(&slow, &c), ["30", "60"]);
        let fast = env().with_display(Display { hdr: true, max_refresh: 144 });
        assert_eq!(rates(&fast, &c), ["30", "60", "120"]);
        let fps = |e: &Env, c: &Config| row(&rows(Section::Recording, e, c), "fps").value(c);
        assert_eq!(fps(&slow, &c), Value::Text("60".into()));
        // A rate this machine does not offer shows as the closest one offered.
        c.video.fps = 144;
        assert_eq!(fps(&fast, &c), Value::Text("120".into()));
        assert_eq!(fps(&slow, &c), Value::Text("60".into()));
        // HDR: offered only while Windows shows it.
        c.video.hdr = "keep_hdr".into();
        assert!(!has(Section::Recording, &c, "hdr"));
        assert!(!has(Section::Image, &c, "image:defaults:image_hdr"));
        let on = rows(Section::Recording, &fast, &c);
        assert!(row(&on, "hdr").enabled());
        assert_eq!(row(&on, "hdr").value(&c), Value::Bool(true));
        // Only hybrid MP4, which older settings read as.
        c.video.container = "mp4_fragmented".into();
        let container = row(&on, "container");
        assert_eq!(container.value(&c), Value::Text("mp4_hybrid".into()));
        assert!(matches!(&container.kind, Kind::Choice(o) if o.len() == 3));
    }

    #[test]
    fn the_split_is_a_kind_and_an_amount() {
        let mut c = Config::default();
        assert!(!has(Section::Recording, &c, "split_amount"));
        let video = rows(Section::Recording, &env(), &c);
        let kind = row(&video, "split");
        kind.apply(&mut c, Value::Text("size".into())).unwrap();
        assert_eq!(c.video.split.mode, "size:2048");
        let video = rows(Section::Recording, &env(), &c);
        let (kind, amount) = (row(&video, "split"), row(&video, "split_amount"));
        amount.apply(&mut c, Value::Int(4096)).unwrap();
        assert_eq!(c.video.split.mode, "size:4096");
        kind.apply(&mut c, Value::Text("duration".into())).unwrap();
        assert_eq!(c.video.split.mode, "duration:10");
        assert_eq!(kind.value(&c), Value::Text("duration".into()));
        kind.apply(&mut c, Value::Text("off".into())).unwrap();
        assert_eq!(c.video.split.mode, "off");
        // Each kind finds its amount again.
        kind.apply(&mut c, Value::Text("size".into())).unwrap();
        assert_eq!(c.video.split.mode, "size:4096");
    }

    #[test]
    fn every_folder_sits_on_its_own_page() {
        let mut c = Config::default();
        // The replay's settings show while it runs.
        c.replay.enabled = true;
        for (section, id) in
            [(Section::Image, "dir_images"), (Section::Recording, "dir_videos"), (Section::Replay, "dir_replays")]
        {
            let page = rows(section, &env(), &c);
            assert!(matches!(row(&page, id).kind, Kind::Folder), "{id}");
        }
        let video = rows(Section::Recording, &env(), &c);
        row(&video, "dir_videos").apply(&mut c, Value::Text(" D:\\Clips ".into())).unwrap();
        assert_eq!(c.paths.videos, "D:\\Clips");
        // Each kind names its files its own way; a template cannot be emptied.
        let image = rows(Section::Image, &env(), &c);
        row(&image, "template:images").apply(&mut c, Value::Text("{app}-{time}".into())).unwrap();
        row(&video, "template:videos").apply(&mut c, Value::Text(" ".into())).unwrap();
        assert_eq!(c.paths.naming.images.template, "{app}-{time}");
        assert_eq!(c.paths.naming.videos.template, "{app} {date} {time}");
        let replay = rows(Section::Replay, &env(), &c);
        assert!(replay.iter().any(|r| r.id == "template:replays"));
        let general = rows(Section::General, &env(), &c);
        assert!(!general.iter().any(|r| r.id.starts_with("template")));
    }

    #[test]
    fn image_overrides_start_off_and_follow_the_effective_format() {
        use vixeeny_common::{config::ImageKind, ipc::ActionId};
        let mut c = Config::default();
        c.image.defaults.format = "png".into();
        for kind in ImageKind::ALL {
            assert!(!c.image.get(kind).custom_encoding && !c.image.get(kind).custom_name);
            assert_eq!(c.image.effective(kind).format, "png");
        }
        let page = rows(Section::Image, &env(), &c);
        assert!(!page.iter().any(|r| r.id == "image:window:image_format"));
        row(&page, "image:window:custom_name").apply(&mut c, Value::Bool(true)).unwrap();
        let page = rows(Section::Image, &env(), &c);
        assert_eq!(row(&page, "image:window:template").suffix, ".png");
        row(&page, "image:window:template").apply(&mut c, Value::Text("Window {date}".into())).unwrap();
        assert_eq!(c.image_template(ActionId::CaptureWindow), "Window {date}");
        row(&page, "image:window:custom_encoding").apply(&mut c, Value::Bool(true)).unwrap();
        let page = rows(Section::Image, &env(), &c);
        assert_eq!(row(&page, "image:window:template").suffix, ".jpg");
        assert_eq!(c.image.for_action(ActionId::CaptureWindow).format, "jpeg");
        assert_eq!(c.image.for_action(ActionId::CaptureFullscreen).format, "png");
    }

    #[test]
    fn capture_settings_are_independent_and_reset_together() {
        use vixeeny_common::config::ImageKind;
        let mut config = Config::default();
        for (index, kind) in ImageKind::ALL.into_iter().enumerate() {
            config.image.get_mut(kind).custom_encoding = true;
            let page = rows(Section::Image, &env(), &config);
            for (id, value) in [("clipboard", false), ("keep_copy", false), ("image_cursor", true)] {
                row(&page, &format!("image:{}:{id}", kind.name())).apply(&mut config, Value::Bool(value)).unwrap();
            }
            row(&page, &format!("image:{}:jpeg_quality", kind.name()))
                .apply(&mut config, Value::Int(50 + index as i64))
                .unwrap();
            let profile = config.image.get(kind);
            assert!(!profile.copy_to_clipboard && !profile.keep_copy && profile.show_cursor);
            assert_eq!(profile.jpeg.quality, 50 + index as u8);
            for other in ImageKind::ALL.into_iter().skip(index + 1) {
                assert_eq!(config.image.get(other), &vixeeny_common::config::Image::default());
            }
        }
        assert!(differs(Section::Image, &config));
        reset(Section::Image, &mut config);
        assert_eq!(config.image, Config::default().image);
    }

    #[test]
    fn overlays_have_previews_and_visibility_precedes_position() {
        let page = rows(Section::Overlay, &env(), &Config::default());
        let index = |id| page.iter().position(|r| r.id == id).unwrap();
        assert_eq!(index("preview_widget"), index("widget") + 1);
        assert_eq!(index("widget_display"), index("preview_widget") + 1);
        assert!(index("widget_display") < index("widget_corner"));
        assert!(row(&page, "widget_corner").info.is_empty());
        for id in ["preview_side", "preview_widget"] {
            assert!(matches!(row(&page, id).kind, Kind::Button));
        }
    }

    #[test]
    fn file_extensions_follow_the_formats_without_changing_templates() {
        let mut config = Config::default();
        config.image.window.format = "png".into();
        let page = rows(Section::Image, &env(), &config);
        assert_eq!(row(&page, "template:images").suffix, ".jpg");
        let before = config.paths.naming.images.template.clone();
        assert_eq!(row(&page, "template:images").value(&config), Value::Text(before));
        config.video.container = "mkv".into();
        assert_eq!(row(&rows(Section::Recording, &env(), &config), "template:videos").suffix, ".mkv");
    }

    #[test]
    fn each_image_format_offers_its_encoder_options() {
        let mut c = Config::default();
        let ids = |c: &Config| -> Vec<String> { rows(Section::Image, &env(), c).into_iter().map(|r| r.id).collect() };
        for (format, own) in [
            ("png", &["image:defaults:png_compression", "image:defaults:png_optimize"][..]),
            ("jpeg", &["image:defaults:jpeg_quality"]),
            ("webp", &["image:defaults:webp_lossless", "image:defaults:webp_effort"]),
            ("avif", &["image:defaults:avif_quality", "image:defaults:avif_depth", "image:defaults:avif_speed"]),
            ("jxl", &["image:defaults:jxl_lossless", "image:defaults:jxl_effort"]),
        ] {
            c.image.defaults.format = format.into();
            let ids = ids(&c);
            for id in own {
                assert!(ids.iter().any(|i| i == id), "{format}: {id}");
            }
            // Only its own.
            let others = ids
                .iter()
                .filter(|i| {
                    i.starts_with("image:defaults:") && !i.trim_start_matches("image:defaults:").starts_with(format)
                })
                .filter(|i| {
                    ["png", "jpeg", "webp", "avif", "jxl"]
                        .iter()
                        .any(|f| i.trim_start_matches("image:defaults:").starts_with(f))
                })
                .count();
            assert_eq!(others, 0, "{format}");
        }
        // A lossy quality only when it is not lossless.
        c.image.defaults.format = "webp".into();
        assert!(!ids(&c).iter().any(|i| i == "image:defaults:webp_quality"));
        c.image.defaults.webp.lossless = false;
        assert!(ids(&c).iter().any(|i| i == "image:defaults:webp_quality"));
    }

    #[test]
    fn the_replay_keeps_its_duration_and_storage() {
        let mut c = Config::default();
        // Only the switch while the replay is off.
        let off = rows(Section::Replay, &env(), &c);
        assert_eq!(off.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["replay_enabled"]);
        c.replay.enabled = true;
        let replay = rows(Section::Replay, &env(), &c);
        // Where its files go comes right under its switch, as on the video page.
        assert_eq!(replay[1].id, "h_folder");
        // Three minutes until changed.
        let duration = row(&replay, "replay_duration");
        assert_eq!((duration.value(&c), duration.unit.as_str()), (Value::Int(3), "min"));
        duration.apply(&mut c, Value::Int(90)).unwrap();
        assert_eq!(c.replay.duration_seconds, 3600);
        // The amount stays in the new unit.
        row(&replay, "replay_unit").apply(&mut c, Value::Text("seconds".into())).unwrap();
        assert_eq!(c.replay.duration_seconds, 60);
        let replay = rows(Section::Replay, &env(), &c);
        row(&replay, "replay_duration").apply(&mut c, Value::Int(5)).unwrap();
        assert_eq!(c.replay.duration_seconds, 5);
        assert_eq!(row(&replay, "replay_duration").unit, "s");
        row(&replay, "replay_storage").apply(&mut c, Value::Text("disk".into())).unwrap();
        assert_eq!(c.replay.storage, "disk");
        // No game open: said in the list.
        let open = env().with_open(["games".to_owned()].into());
        assert!(rows(Section::Replay, &open, &c).iter().any(|r| r.id == "no_games"));
    }

    #[test]
    fn the_disk_alerts_are_a_switch_and_a_share_each() {
        let mut c = Config::default();
        c.replay.enabled = true;
        let video = rows(Section::Recording, &env(), &c);
        row(&video, "disk_alert_percent:videos").apply(&mut c, Value::Int(150)).unwrap();
        assert_eq!(c.disk_alert.recordings.threshold(), 99);
        row(&video, "disk_alert:videos").apply(&mut c, Value::Bool(false)).unwrap();
        assert!(!has(Section::Recording, &c, "disk_alert_percent:videos"));
        assert_eq!(c.disk_alert.recordings.threshold(), 0);
        // Kept for when it is on again; the replay's is its own.
        assert_eq!(c.disk_alert.recordings.percent, 99);
        assert_eq!(c.disk_alert.replays.threshold(), 5);
        assert!(has(Section::Replay, &c, "disk_alert_percent:replays"));
        assert!(!has(Section::General, &c, "disk_alert:videos"));
    }

    #[test]
    fn the_replay_has_video_settings_of_its_own() {
        let mut c = Config::default();
        c.replay.enabled = true;
        c.video.fps = 30;
        let replay = rows(Section::Replay, &with_nvenc(), &c);
        // Its own: read and written apart from the videos'.
        let fps = row(&replay, "replay:fps");
        assert_eq!(fps.value(&c), Value::Text("60".into()));
        fps.apply(&mut c, Value::Text("30".into())).unwrap();
        assert_eq!(c.replay.video.fps, 30);
        // A replay is not cut into parts; its sound is its own too, one track whatever it records.
        assert!(!replay.iter().any(|r| r.id == "replay:split"));
        c.video.audio.bitrate_kbps = 320;
        row(&replay, "replay:audio_bitrate").apply(&mut c, Value::Text("128".into())).unwrap();
        assert_eq!((c.replay_video().audio.bitrate_kbps, c.video.audio.bitrate_kbps), (128, 320));
        row(&replay, "replay:mic_on").apply(&mut c, Value::Bool(true)).unwrap();
        assert!(
            c.replay.video.audio.sources.iter().any(|s| s == "mic")
                && !c.video.audio.sources.iter().any(|s| s == "mic")
        );
        let replay = rows(Section::Replay, &with_nvenc(), &c);
        assert_eq!(row(&replay, "replay:audio_routing").value(&c), Value::Text("mix_all".into()));
        // The same as the videos: none of them, and what was set stays.
        row(&replay, "replay_same_video").apply(&mut c, Value::Bool(true)).unwrap();
        assert!(!has(Section::Replay, &c, "replay:fps"));
        assert_eq!(c.replay_video().fps, 30);
        // Still on the graphics card when the videos are not.
        c.video.encoder_kind = "software".into();
        assert_eq!(c.replay_video().encoder_kind, "hardware");
        // Its expanders are its own.
        c.replay.same_as_video = false;
        c.replay.video.preset = "custom".into();
        let opened = env().with_open(["replay:expert_video".to_owned()].into());
        let replay = rows(Section::Replay, &opened, &c);
        assert!(replay.iter().any(|r| r.id == "replay:chroma"));
        assert!(!rows(Section::Recording, &opened, &c).iter().any(|r| r.id == "chroma"));
        assert!(row(&replay, "replay:encoder").info.starts_with("AV1"));
    }

    fn with_nvenc() -> Env {
        use vixeeny_encode::probe::{EncoderProbe, FormatProbe, ProbeResult};
        use vixeeny_encode::registry::Chroma;
        let probe = ProbeResult {
            encoders: vec![EncoderProbe {
                id: "nvenc_h264".into(),
                adapter: Some(0),
                formats: vec![FormatProbe { depth: 8, chroma: Chroma::C420, uhd: true, hdr: false }],
                tools: None,
                max: None,
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
        // Hardware by default; nothing listed while the probe runs.
        let detecting = env().with_probe(None, true);
        assert!(encoder_options(&rows(Section::Recording, &detecting, &c)).is_empty());
        // No hardware encoder here: the software ones, and the kind says so.
        let none = rows(Section::Recording, &env(), &c);
        assert!(encoder_options(&none).contains(&"libx264".to_owned()));
        let kind = row(&none, "encoder_kind");
        assert!(kind.greyed && !kind.warning.is_empty());
        assert_eq!(kind.value(&c), Value::Text("software".into()));
        let e = with_nvenc();
        let video = rows(Section::Recording, &e, &c);
        assert_eq!(encoder_options(&video), ["nvenc_h264"]);
        // `auto` shows the encoder it resolves to.
        assert_eq!(row(&video, "encoder").value(&c), Value::Text("nvenc_h264".into()));
        // Software lists the software encoders only, and resets the choice.
        row(&video, "encoder_kind").apply(&mut c, Value::Text("software".into())).unwrap();
        let video = rows(Section::Recording, &e, &c);
        let software = encoder_options(&video);
        assert!(software.contains(&"libx264".to_owned()));
        assert!(!software.contains(&"nvenc_h264".to_owned()));
        assert_eq!(c.video.encoder, "auto");
    }

    #[test]
    fn the_video_page_speaks_in_pixels_and_offers_16_9_on_a_wide_screen() {
        use crate::machine::{Machine, Screen};
        let screen = |width, height| Machine {
            screens: vec![Screen { number: 1, primary: true, name: "Screen".into(), width, height, hz: 144 }],
            ..Machine::default()
        };
        let options = |rows_: &[Row], id: &str| match &row(rows_, id).kind {
            Kind::Choice(o) => o.iter().map(|o| o.label.clone()).collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let mut c = Config::default();
        let uhd = env().with_machine(screen(3840, 2160));
        let video = rows(Section::Recording, &uhd, &c);
        assert_eq!(
            options(&video, "resolution"),
            ["3840 × 2160 (4K UHD)", "2560 × 1440 (QHD)", "1920 × 1080 (Full HD)", "1280 × 720 (HD)", "854 × 480 (SD)"]
        );
        // The screen's own height, set by name (or a larger screen's): the screen's size, once.
        c.video.resolution = "2160p".into();
        let video = rows(Section::Recording, &uhd, &c);
        assert_eq!(options(&video, "resolution").len(), 5);
        let qhd = env().with_machine(screen(2560, 1440));
        let video = rows(Section::Recording, &qhd, &c);
        assert_eq!(options(&video, "resolution")[0], "2560 × 1440 (QHD)");
        assert_eq!(options(&video, "resolution").len(), 4);
        assert!(matches!(row(&video, "resolution").value(&c), Value::Text(v) if v == "source"));
        c.video.resolution = "source".into();
        // 16:9: nothing to choose.
        assert!(!video.iter().any(|r| r.id == "aspect"));
        assert!(!video.iter().any(|r| r.id == "ten_bit"));
        // An ultrawide screen can be recorded as 16:9, and the sizes follow.
        let wide = env().with_machine(screen(3440, 1440));
        let video = rows(Section::Recording, &wide, &c);
        assert_eq!(options(&video, "aspect"), ["The screen's (21:9)", "16:9, cropped in the middle"]);
        assert_eq!(options(&video, "resolution")[1], "2580 × 1080");
        row(&video, "aspect").apply(&mut c, Value::Text("16:9".into())).unwrap();
        let video = rows(Section::Recording, &wide, &c);
        assert_eq!(options(&video, "resolution")[..2], ["2560 × 1440 (QHD)", "1920 × 1080 (Full HD)"]);
    }

    #[test]
    fn the_custom_preset_starts_from_the_one_left_then_keeps_its_settings() {
        let mut c = Config::default();
        c.video.preset = "quality".into();
        let env = with_nvenc();
        let video = rows(Section::Recording, &env, &c);
        row(&video, "preset").apply(&mut c, Value::Text("custom".into())).unwrap();
        assert_eq!(c.video.preset, "custom");
        // NVENC's best quality: CQ 19, not the scale's 23.
        assert_eq!(c.video.params["rc.quality"], "19");
        assert_eq!(c.video.params["multipass"], "qres");
        // Set by hand, then another preset and back: what was set by hand is kept.
        c.video.params.insert("rc.quality".into(), "21".into());
        let video = rows(Section::Recording, &env, &c);
        row(&video, "preset").apply(&mut c, Value::Text("balanced".into())).unwrap();
        row(&video, "preset").apply(&mut c, Value::Text("custom".into())).unwrap();
        assert_eq!(c.video.params["rc.quality"], "21");
    }

    #[test]
    fn each_kind_of_encoder_keeps_the_one_chosen() {
        let mut c = Config::default();
        let env = with_nvenc();
        let kind = |c: &mut Config, v: &str| {
            row(&rows(Section::Recording, &env, c), "encoder_kind").apply(c, Value::Text(v.into())).unwrap();
        };
        c.video.encoder = "nvenc_h264".into();
        kind(&mut c, "software");
        assert_eq!(c.video.encoder, "auto");
        row(&rows(Section::Recording, &env, &c), "encoder").apply(&mut c, Value::Text("libx265".into())).unwrap();
        kind(&mut c, "hardware");
        assert_eq!(c.video.encoder, "nvenc_h264");
        kind(&mut c, "software");
        assert_eq!(c.video.encoder, "libx265");
    }

    #[test]
    fn each_encoder_keeps_its_own_custom_settings() {
        let mut c = Config::default();
        let env = with_nvenc();
        let set = |c: &mut Config, id: &str, v: Value| {
            row(&rows(Section::Recording, &env, c), id).apply(c, v).unwrap();
        };
        set(&mut c, "preset", Value::Text("custom".into()));
        set(&mut c, "p:rc.quality", Value::Int(30));
        assert_eq!(c.video.params["rc.quality"], "30");
        // Another kind, another encoder: its own options, from the profile advised.
        set(&mut c, "encoder_kind", Value::Text("software".into()));
        let x264 = c.video.params.clone();
        assert_ne!(x264.get("rc.quality").map(String::as_str), Some("30"));
        set(&mut c, "p:rc.quality", Value::Int(18));
        set(&mut c, "encoder", Value::Text("libx265".into()));
        assert_ne!(c.video.params.get("rc.quality").map(String::as_str), Some("18"));
        // Back to each: what was set for it.
        set(&mut c, "encoder", Value::Text("libx264".into()));
        assert_eq!(c.video.params["rc.quality"], "18");
        set(&mut c, "encoder_kind", Value::Text("hardware".into()));
        assert_eq!(c.video.params["rc.quality"], "30");
    }

    #[test]
    fn expert_settings_stay_folded_until_opened() {
        let mut c = Config::default();
        c.video.preset = "custom".into();
        let closed = rows(Section::Recording, &with_nvenc(), &c);
        assert!(matches!(row(&closed, "expert_video").kind, Kind::Expander));
        assert!(closed.iter().any(|r| r.id == "p:preset"));
        assert!(!closed.iter().any(|r| r.id == "p:multipass" || r.id == "chroma"));
        let open = with_nvenc().with_open(["expert_video".to_owned()].into());
        let opened = rows(Section::Recording, &open, &c);
        assert!(opened.iter().any(|r| r.id == "p:multipass"));
        assert!(opened.iter().any(|r| r.id == "chroma"));
    }

    #[test]
    fn hdr_is_one_switch() {
        let mut c = Config::default();
        let env = env().with_display(Display { hdr: true, max_refresh: 60 });
        let images = rows(Section::Image, &env, &c);
        let hdr = row(&images, "image:defaults:image_hdr");
        assert_eq!(hdr.value(&c), Value::Bool(false));
        assert_eq!(c.image.defaults.hdr, "tonemap_sdr");
        hdr.apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.image.defaults.hdr, "keep_hdr");
        assert!(hdr.enabled());
        // WebP cannot keep it: the switch is greyed out, its "i" says why.
        c.image.defaults.format = "webp".into();
        let images = rows(Section::Image, &env, &c);
        let greyed = row(&images, "image:defaults:image_hdr");
        assert!(!greyed.enabled());
        assert!(greyed.info.contains("WebP"), "{}", greyed.info);
        assert!(greyed.apply(&mut c, Value::Bool(false)).is_err());
        let video = rows(Section::Recording, &env, &c);
        row(&video, "hdr").apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.video.hdr, "keep_hdr");
        row(&video, "hdr").apply(&mut c, Value::Bool(false)).unwrap();
        assert_eq!(c.video.hdr, "tonemap_sdr");
    }

    #[test]
    fn presets_are_quality_balanced_performance_or_custom_and_custom_shows_the_encoder_options() {
        let mut c = Config::default();
        c.video.encoder_kind = "software".into();
        let ids =
            |c: &Config| -> Vec<String> { rows(Section::Recording, &env(), c).into_iter().map(|r| r.id).collect() };
        assert!(!ids(&c).iter().any(|i| i.starts_with("p:")));
        let preset = |c: &Config| row(&rows(Section::Recording, &env(), c), "preset").value(c);
        assert_eq!(preset(&c), Value::Text("balanced".into()));
        c.video.preset = "performance".into();
        assert_eq!(preset(&c), Value::Text("performance".into()));
        // The compact profile of before 1.0 is the balanced one; older presets are shown as custom.
        c.video.preset = "small".into();
        assert_eq!(preset(&c), Value::Text("balanced".into()));
        c.video.preset = "fast".into();
        let video = rows(Section::Recording, &env(), &c);
        assert_eq!(row(&video, "preset").value(&c), Value::Text("custom".into()));
        c.video.preset = "custom".into();
        let video = rows(Section::Recording, &env(), &c);
        assert!(ids(&c).iter().any(|i| i == "p:rc.quality"));
        // A custom option is stored by its key and read back (the default until set).
        let crf = row(&video, "p:rc.quality");
        assert_eq!(crf.value(&c), Value::Int(23));
        crf.apply(&mut c, Value::Int(18)).unwrap();
        assert_eq!(c.video.params["rc.quality"], "18");
        assert_eq!(crf.value(&c), Value::Int(18));
        // Values are shown by name.
        let Kind::Choice(presets) = &row(&video, "p:preset").kind else { panic!("the preset is a list") };
        assert_eq!(presets[0].label, "Ultra fast");
        // The fields follow the rate mode: a bitrate replaces the quality.
        row(&video, "p:rc.mode").apply(&mut c, Value::Text("vbr".into())).unwrap();
        let after = ids(&c);
        assert!(!after.iter().any(|i| i == "p:rc.quality"));
        assert!(after.iter().any(|i| i == "p:rc.bitrate"));
        assert!(after.iter().any(|i| i == "p:rc.maxrate"));
    }

    #[test]
    fn the_sound_to_record_is_one_choice_and_the_microphone_another() {
        let mut c = Config::default();
        // By default, the recorded program only; the test starts from all the PC sound.
        assert_eq!(c.video.audio.sources, [TARGET_SOURCE]);
        c.video.audio.capture = "system".into();
        c.video.audio.sources = vec!["system".into()];
        let e = env().with_audio(AudioDevices {
            outputs: vec![AudioEntry { id: "{out-1}".into(), name: "Headset".into(), icon: None }],
            inputs: vec![AudioEntry { id: "{in-1}".into(), name: "Blue Yeti".into(), icon: None }],
            programs: vec![AudioEntry { id: "spotify.exe".into(), name: "Spotify".into(), icon: None }],
            games: Vec::new(),
        });
        let audio = rows(Section::Recording, &e, &c);
        // All the PC sound; the microphone is off, its choice left out.
        let capture = |c: &Config| row(&rows(Section::Recording, &e, c), "audio_capture").value(c);
        assert_eq!(capture(&c), Value::Text("system".into()));
        assert_eq!(row(&audio, "mic_on").value(&c), Value::Bool(false));
        assert!(!has_in(&audio, "mic_device"));
        assert!(!has_in(&audio, "mic_volume"));
        // One track: nothing to route.
        assert!(!has_in(&audio, "audio_routing"));
        // On: the Windows default, then the one picked.
        row(&audio, "mic_on").apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.video.audio.sources, ["system", "mic"]);
        let audio = rows(Section::Recording, &e, &c);
        assert!(row(&audio, "mic_volume").meter);
        assert_eq!(row(&audio, "mic_volume").number_text(100, Lang::Fr), "100\u{a0}%");
        match &row(&audio, "audio_routing").kind {
            Kind::Choice(o) => assert_eq!(o.len(), 2),
            _ => panic!("audio_routing is a choice"),
        }
        match &row(&audio, "mic_device").kind {
            Kind::Choice(o) => {
                assert_eq!(o.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Windows default", "Blue Yeti"])
            }
            _ => panic!("mic_device is a choice"),
        }
        row(&audio, "mic_device").apply(&mut c, Value::Text("mic:{in-1}".into())).unwrap();
        // The recorded program: the microphone stays.
        let set = |c: &mut Config, mode: &str| {
            row(&rows(Section::Recording, &e, c), "audio_capture").apply(c, Value::Text(mode.into())).unwrap();
        };
        set(&mut c, "target");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", TARGET_SOURCE]);
        // One output: the first device, then the one picked.
        set(&mut c, "output");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", "out:{out-1}"]);
        let audio = rows(Section::Recording, &e, &c);
        assert_eq!(row(&audio, "audio_output").value(&c), Value::Text("out:{out-1}".into()));
        // Chosen programs: ticked one by one; none ticked is still that choice.
        set(&mut c, "programs");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}"]);
        assert_eq!(capture(&c), Value::Text("programs".into()));
        let audio = rows(Section::Recording, &e, &c);
        assert_eq!(row(&audio, "src:app:spotify.exe").label, "Spotify");
        row(&audio, "src:app:spotify.exe").apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", "app:spotify.exe"]);
        // Another mode, then back: the device and the programs chosen come back.
        c.video.audio.set_aside = vec!["out:{out-2}".into()];
        set(&mut c, "system");
        set(&mut c, "output");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", "out:{out-2}"]);
        set(&mut c, "programs");
        assert_eq!(c.video.audio.sources, ["mic:{in-1}", "app:spotify.exe"]);
        assert_eq!(c.video.audio.set_aside, ["out:{out-2}"]);
        // A ticked program that is closed now stays listed.
        c.video.audio.sources.push("app:game.exe".into());
        let audio = rows(Section::Recording, &e, &c);
        assert_eq!(row(&audio, "src:app:game.exe").label, "game.exe (closed)");
        // Off removes the microphone, whichever it was; on again, it is back.
        row(&audio, "mic_on").apply(&mut c, Value::Bool(false)).unwrap();
        assert_eq!(c.video.audio.sources, ["app:spotify.exe", "app:game.exe"]);
        row(&audio, "mic_on").apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.video.audio.sources, ["app:spotify.exe", "app:game.exe", "mic:{in-1}"]);
        // Older settings without the field: read from their sources.
        let mut old = Config::default();
        old.video.audio.sources = vec!["mic".into()];
        old.video.audio.capture = "system".into();
        assert_eq!(capture(&old), Value::Text("none".into()));
    }

    #[test]
    fn a_page_goes_back_to_how_it_comes_alone() {
        let mut c = Config::default();
        assert!(Section::ALL.iter().all(|s| !differs(*s, &c)));
        c.general.theme = "dark".into();
        c.video.fps = 30;
        c.video.audio.bitrate_kbps = 320;
        c.video.audio.set_aside = vec!["out:{x}".into()];
        assert!(differs(Section::General, &c) && differs(Section::Recording, &c));
        assert!(!differs(Section::Image, &c));
        reset(Section::Recording, &mut c);
        assert_eq!(c.video.fps, Config::default().video.fps);
        // The sound is on the same page.
        assert_eq!(c.video.audio.bitrate_kbps, Config::default().video.audio.bitrate_kbps);
        assert!(!differs(Section::Recording, &c));
        assert_eq!(c.video.audio.set_aside, ["out:{x}"]);
        assert_eq!(c.general.theme, "dark");
        reset_all(&mut c);
        assert_eq!(c.general.theme, Config::default().general.theme);
    }

    #[test]
    fn the_volumes_are_percentages_of_each_side() {
        let mut c = Config::default();
        let audio = rows(Section::Recording, &env(), &c);
        let capture = row(&audio, "capture_volume");
        assert_eq!(capture.value(&c), Value::Int(100));
        capture.apply(&mut c, Value::Int(152)).unwrap();
        assert_eq!(c.video.audio.volumes["capture"], 1.5);
        // As it is: nothing written.
        capture.apply(&mut c, Value::Int(100)).unwrap();
        assert!(c.video.audio.volumes.is_empty());
        c.video.audio.sources.push("mic".into());
        let audio = rows(Section::Recording, &env(), &c);
        row(&audio, "mic_volume").apply(&mut c, Value::Int(60)).unwrap();
        assert_eq!(c.video.audio.volumes["mic"], 0.6);
        // Nothing recorded besides the microphone: no volume for it.
        c.video.audio.capture = "none".into();
        c.video.audio.sources = vec!["mic".into()];
        assert!(!has(Section::Recording, &c, "capture_volume"));
    }

    #[test]
    fn the_replay_can_go_on_after_its_shortcut() {
        let mut c = Config::default();
        c.replay.enabled = true;
        let replay = rows(Section::Replay, &env(), &c);
        let after = row(&replay, "replay_after");
        assert_eq!(after.value(&c), Value::Bool(false));
        assert!(!has_in(&replay, "replay_after_seconds"));
        after.apply(&mut c, Value::Bool(true)).unwrap();
        let replay = rows(Section::Replay, &env(), &c);
        row(&replay, "replay_after_seconds").apply(&mut c, Value::Int(12)).unwrap();
        assert_eq!(c.replay.after(), 12);
        // Off, then on again: the seconds are kept.
        row(&replay, "replay_after").apply(&mut c, Value::Bool(false)).unwrap();
        assert_eq!(c.replay.after(), 0);
        row(&replay, "replay_after").apply(&mut c, Value::Bool(true)).unwrap();
        assert_eq!(c.replay.after(), 12);
        assert_eq!(row(&rows(Section::Overlay, &env(), &c), "overlay_edge").picker, Picker::Edge);
    }

    #[test]
    fn a_search_finds_settings_by_the_start_of_their_words_without_accents() {
        let fr = Env::new(Lang::Fr, "1");
        let mut c = Config::default();
        c.video.preset = "custom".into();
        c.replay.enabled = true;
        assert!(matches("debit", "Débit audio (kbit/s)"));
        assert!(matches("AUDIO deb", "Débit audio (kbit/s)"));
        assert!(!matches("bit", "Débit audio"));
        assert!(!matches("  ", "Débit audio"));
        let found = search("dossier", &fr, &c);
        let pages: Vec<Section> = found.iter().map(|f| f.section).collect();
        assert!(pages.contains(&Section::Image) && pages.contains(&Section::Replay));
        // The folded expert settings are found too.
        let found = search("chroma", &fr, &c);
        assert_eq!(found[0].section, Section::Recording);
        assert!(search("zzz", &fr, &c).is_empty());
    }

    #[test]
    fn bad_video_settings_are_reported() {
        let mut c = Config::default();
        assert!(video_problems(&c, (1920, 1080), Lang::En).is_empty());
        c.video.container = "avi".into();
        let problems = video_problems(&c, (1920, 1080), Lang::Fr);
        assert!(problems.iter().any(|p| p.contains("conteneur")), "{problems:?}");
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
        c.replay.enabled = true;
        c.replay.duration_seconds = 1200;
        // Under the storage's name.
        let hint = |env: &Env, c: &Config| row(&rows(Section::Replay, env, c), "replay_storage").detail.clone();
        assert!(hint(&with_ram(64), &c).ends_with("in RAM"), "{}", hint(&with_ram(64), &c));
        assert!(hint(&with_ram(8), &c).contains("on the disk"), "{}", hint(&with_ram(8), &c));
        c.replay.storage = "disk".into();
        assert!(!hint(&with_ram(64), &c).contains(" in RAM"));
        assert!(hint(&with_ram(64), &c).contains("GB"));
    }

    #[test]
    fn the_replay_starts_on_the_performance_profile_and_the_videos_encoder() {
        let mut c = Config::default();
        c.replay.enabled = true;
        let marked = |rows: &[Row], id: &str| -> Vec<String> {
            let Kind::Choice(presets) = &row(rows, id).kind else {
                panic!("the profile is a choice");
            };
            presets.iter().filter(|o| o.recommended).map(|o| o.value.clone()).collect()
        };
        let replay = rows(Section::Replay, &with_nvenc(), &c);
        assert_eq!(row(&replay, "replay:preset").value(&c), Value::Text("performance".into()));
        assert_eq!(marked(&replay, "replay:preset"), ["performance"]);
        // Not a custom one: no options by hand. The videos advise the balanced one.
        assert!(!replay.iter().any(|r| r.id.starts_with("replay:p:")));
        let video = rows(Section::Recording, &with_nvenc(), &c);
        assert_eq!(marked(&video, "preset"), ["balanced"]);
        // Always on the graphics card: no choice of the kind.
        assert!(!has_in(&replay, "replay:encoder_kind") && has_in(&video, "encoder_kind"));
        // The encoder `auto` stands for is the videos' one.
        let both = with_nvenc_av1();
        let replay = rows(Section::Replay, &both, &c);
        assert_eq!(row(&replay, "replay:encoder").value(&c), Value::Text("nvenc_av1".into()));
        // No hardware encoder: no replay, said beside its switch.
        let none = env().with_probe(Some(vixeeny_encode::probe::ProbeResult::default()), false);
        let replay = rows(Section::Replay, &none, &c);
        assert_eq!(replay.len(), 1);
        assert!(replay[0].greyed && !replay[0].warning.is_empty());
        assert_eq!(replay[0].value(&c), Value::Bool(false));
    }

    fn with_nvenc_av1() -> Env {
        use vixeeny_encode::probe::{EncoderProbe, FormatProbe, ProbeResult};
        use vixeeny_encode::registry::Chroma;
        let probe = |id: &str| EncoderProbe {
            id: id.into(),
            adapter: Some(0),
            formats: vec![FormatProbe { depth: 8, chroma: Chroma::C420, uhd: true, hdr: false }],
            tools: None,
            max: None,
        };
        let probe = ProbeResult { encoders: vec![probe("nvenc_h264"), probe("nvenc_av1")], ..ProbeResult::default() };
        env().with_probe(Some(probe), false)
    }

    #[test]
    fn settings_explain_themselves_and_recommend_only_numbers() {
        let mut c = Config::default();
        c.video.preset = "custom".into();
        let video = rows(Section::Recording, &with_nvenc(), &c);
        assert!(row(&video, "encoder_kind").info.starts_with("Hardware:"));
        assert!(!row(&video, "encoder_kind").info.contains("Recommended"));
        assert!(row(&video, "p:preset").info.contains("Recommended here: P4"));
        assert!(row(&video, "p:preset").info.contains("P6 and P7"));
        assert!(row(&video, "p:rc.quality").info.contains("Recommended: 24"));
        // The speed that keeps up is marked in the list too, and only it.
        let Kind::Choice(presets) = &row(&video, "p:preset").kind else {
            panic!("the preset is a choice");
        };
        let marked: Vec<_> = presets.iter().filter(|o| o.recommended).map(|o| o.label.as_str()).collect();
        assert_eq!(marked, ["P4"]);
        // A setting with nothing to add has no "i".
        assert!(row(&video, "video_cursor").info.is_empty());
        assert!(row(&video, "preset").info.contains("Performance"));
        c.image.defaults.format = "jpeg".into();
        let image = rows(Section::Image, &env(), &c);
        assert!(row(&image, "image:defaults:image_format").info.is_empty());
        assert!(row(&image, "image:defaults:jpeg_quality").info.is_empty());
    }
}
