// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings window (plan 5.13). It owns a copy of the [`Config`], applies every change the
//! moment it is made (the host is told through [`SettingsPanel::on_change`]), and leaves what
//! needs the OS (folders to browse and open, the hardware list) to the host.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::ActionId;
use vixeeny_encode::probe::ProbeResult;
use vixeeny_settings::shortcuts::{self, Refusal};
use vixeeny_settings::{
    AudioDevices, Display, Env, Found, Kind, Machine, Picker, Row, Section, Value, rows, search,
    video_problems,
};

use crate::theme::{self, Look};
use crate::{
    LineItem, PcLine, SearchHit, SettingGroup, SettingRow, SettingsWindow, ShortcutKey,
    ShortcutRow, UiTexts,
};

/// One line of the about page.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub text: String,
    pub detail: String,
}

/// Where the update stands, for the card of the updates page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateStage {
    /// Up to date, or not checked yet.
    #[default]
    Idle,
    Checking,
    Available,
    Downloading,
    /// Downloaded and verified: a restart installs it.
    Ready,
    Failed,
    Restarting,
}

/// What the update card shows.
#[derive(Debug, Clone, Default)]
pub struct UpdateView {
    pub stage: UpdateStage,
    pub title: String,
    pub detail: String,
    /// 0.0–1.0 while downloading.
    pub progress: f32,
    /// The label of the button (empty: none).
    pub action: String,
}

fn stage_index(stage: UpdateStage) -> i32 {
    match stage {
        UpdateStage::Idle => 0,
        UpdateStage::Checking => 1,
        UpdateStage::Available => 2,
        UpdateStage::Downloading => 3,
        UpdateStage::Ready => 4,
        UpdateStage::Failed => 5,
        UpdateStage::Restarting => 6,
    }
}

fn show_update(w: &SettingsWindow, view: &UpdateView) {
    w.set_update_state(stage_index(view.stage));
    w.set_update_title(view.title.as_str().into());
    w.set_update_detail(view.detail.as_str().into());
    w.set_update_progress(view.progress);
    w.set_update_action(view.action.as_str().into());
}

/// A key press while a shortcut is being recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    Cancel,
    Clear,
    /// `Ctrl+Shift+KeyR`, in the notation of the settings file.
    Combination(String),
}

/// The keys held while a shortcut is recorded: the modifiers in the order they went down, then
/// the key. Letting go of a key ends the shortcut; Escape empties the slot.
#[derive(Debug, Default)]
pub struct Chord {
    mods: Vec<&'static str>,
    key: Option<String>,
}

/// What a key did to the shortcut being recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordStep {
    /// Still held: the keys so far, `Ctrl+Shift` or `Ctrl+Shift+KeyR`.
    Held(String),
    Done(Captured),
}

impl Chord {
    /// A key (its `KeyboardEvent.code` name, `ControlLeft`, `KeyR`) went down.
    pub fn press(&mut self, code: &str) -> ChordStep {
        if code == "Escape" {
            *self = Self::default();
            return ChordStep::Done(Captured::Clear);
        }
        match modifier(code) {
            Some(name) if !self.mods.contains(&name) => self.mods.push(name),
            Some(_) => {}
            None => self.key = Some(code.to_owned()),
        }
        ChordStep::Held(self.text())
    }

    /// A key went up: the shortcut is complete once it has a key. Letting go of modifiers alone
    /// forgets them (a key that only reports its release, Print Screen, still counts).
    pub fn release(&mut self, code: &str) -> ChordStep {
        match modifier(code) {
            Some(name) if self.key.is_none() => {
                self.mods.retain(|m| *m != name);
                return ChordStep::Held(self.text());
            }
            Some(_) => {}
            None if self.key.is_none() => self.key = Some(code.to_owned()),
            None => {}
        }
        let text = self.text();
        *self = Self::default();
        ChordStep::Done(Captured::Combination(text))
    }

    fn text(&self) -> String {
        let mut parts: Vec<&str> = self.mods.clone();
        parts.extend(self.key.as_deref());
        parts.join("+")
    }
}

/// The modifier a key is, both sides alike.
fn modifier(code: &str) -> Option<&'static str> {
    match code {
        "ControlLeft" | "ControlRight" => Some("Ctrl"),
        "AltLeft" | "AltRight" => Some("Alt"),
        "ShiftLeft" | "ShiftRight" => Some("Shift"),
        "SuperLeft" | "SuperRight" | "MetaLeft" | "MetaRight" => Some("Win"),
        _ => None,
    }
}

type ChangeFn = Box<dyn Fn(&Config)>;
type BrowseFn = Box<dyn Fn(&str) -> Option<String>>;
type PageFn = Box<dyn Fn(Section, &str, &str)>;
type AudioFn = std::sync::Arc<dyn Fn() -> AudioDevices + Send + Sync>;
type MeterFn = Box<dyn Fn(Option<String>)>;

struct State {
    config: RefCell<Config>,
    env: RefCell<Env>,
    os_locale: Option<String>,
    version: String,
    rows: RefCell<Vec<Row>>,
    /// The hardware probe, once known, and whether it is still running.
    probe: RefCell<(Option<ProbeResult>, bool)>,
    audio: RefCell<AudioDevices>,
    machine: RefCell<Machine>,
    /// The expanders that are open (closed again when the window closes).
    open: RefCell<std::collections::BTreeSet<String>>,
    /// Lists the devices and programs (slow: the programs' icons); run on another thread.
    audio_source: RefCell<Option<AudioFn>>,
    display: Cell<Display>,
    /// The shortcut being recorded: `(row, slot)`.
    recording: Cell<Option<(usize, usize)>>,
    chord: RefCell<Chord>,
    on_recording: RefCell<Box<dyn Fn(bool)>>,
    on_change: RefCell<ChangeFn>,
    on_browse: RefCell<BrowseFn>,
    on_page_action: RefCell<PageFn>,
    on_section: RefCell<Box<dyn Fn(Section)>>,
    /// What answers the search now.
    found: RefCell<Vec<Found>>,
    /// The shortcuts the daemon could not register.
    taken: RefCell<Vec<String>>,
    /// The microphone whose level the audio page shows (`None`: no meter on screen).
    metered: RefCell<Option<String>>,
    on_meter: RefCell<MeterFn>,
}

/// A way to update the window from another thread (a detection, a download that takes a while).
#[derive(Clone)]
pub struct PanelHandle(slint::Weak<SettingsWindow>);

impl PanelHandle {
    pub fn set_lines(&self, lines: Vec<Line>) {
        let _ = self
            .0
            .upgrade_in_event_loop(move |w| w.set_lines(line_model(lines, None)));
    }

    pub fn set_extra(&self, text: String) {
        let _ = self
            .0
            .upgrade_in_event_loop(move |w| w.set_extra(text.into()));
    }

    pub fn set_update(&self, view: UpdateView) {
        let _ = self
            .0
            .upgrade_in_event_loop(move |w| show_update(&w, &view));
    }

    /// How loud the microphone is now, 0 to 1 (from the thread that measures it).
    pub fn set_mic_level(&self, level: f32) {
        let _ = self
            .0
            .upgrade_in_event_loop(move |w| w.set_mic_level(level));
    }

    /// Closes the window (the program then ends).
    pub fn close(&self) {
        let _ = self.0.upgrade_in_event_loop(|w| {
            let _ = w.hide();
        });
    }
}

fn line_model(lines: Vec<Line>, selected: Option<usize>) -> ModelRc<LineItem> {
    let items: Vec<LineItem> = lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| LineItem {
            text: l.text.into(),
            detail: l.detail.into(),
            selected: selected == Some(i),
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(items)))
}

pub struct SettingsPanel {
    window: SettingsWindow,
    state: Rc<State>,
}

/// The sheet of this computer, for the general page.
fn pc_lines(lines: &[vixeeny_settings::machine::Line]) -> ModelRc<PcLine> {
    ModelRc::from(Rc::new(VecModel::from(
        lines
            .iter()
            .map(|l| PcLine {
                label: l.label.as_str().into(),
                value: l.value.as_str().into(),
            })
            .collect::<Vec<_>>(),
    )))
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )))
}

fn ui_texts(lang: Lang) -> UiTexts {
    let t = |k| SharedString::from(tr(k, lang));
    UiTexts {
        github: t(Key::UiGithub),
        logs: t(Key::UiLogs),
        licenses: t(Key::UiLicenses),
        copy_info: t(Key::UiCopyInfo),
        change: t(Key::UiChange),
        show: t(Key::UiShow),
        remove: t(Key::UiRemove),
        add_shortcut: t(Key::UiAddShortcut),
        press_keys: t(Key::UiPressKeys),
        no_shortcut: t(Key::UiNoShortcut),
        keys_help: t(Key::UiKeysHelp),
        restart: t(Key::UpdateRestart),
    }
}

/// The label of an action in the shortcuts table.
pub fn action_label(action: ActionId, lang: Lang) -> &'static str {
    tr(
        match action {
            ActionId::CaptureRegion => Key::ActCaptureRegion,
            ActionId::CaptureWindow => Key::ActCaptureWindow,
            ActionId::CaptureFullscreen => Key::ActCaptureFullscreen,
            ActionId::CaptureAllMonitors => Key::ActCaptureAll,
            ActionId::CaptureScrolling => Key::ActCaptureScrolling,
            ActionId::RecordToggle => Key::ActRecordToggle,
            ActionId::RecordPause => Key::ActRecordPause,
            ActionId::ReplayToggle | ActionId::ReplayWatch => Key::ActReplayToggle,
            ActionId::ReplaySave => Key::ActReplaySave,
            ActionId::OverlayToggle => Key::ActOverlay,
            ActionId::OpenSettings => Key::ActSettings,
        },
        lang,
    )
}

/// The rows as groups: each header starts one.
fn grouped(rows: &[Row], config: &Config) -> Vec<Vec<SettingRow>> {
    let mut groups: Vec<Vec<SettingRow>> = vec![Vec::new()];
    for row in rows {
        if matches!(row.kind, Kind::Header) {
            groups.push(Vec::new());
        } else if let Some(list) = groups.last_mut() {
            list.push(row_model(row, config));
        }
    }
    groups.retain(|rows| !rows.is_empty());
    groups
}

/// The same row as far as the eye can tell (the option lists compared by content).
fn same_row(a: &SettingRow, b: &SettingRow) -> bool {
    let options = |r: &SettingRow| r.options.iter().collect::<Vec<_>>();
    a.id == b.id
        && a.label == b.label
        && a.hint == b.hint
        && a.kind == b.kind
        && a.enabled == b.enabled
        && a.on == b.on
        && a.text == b.text
        && a.num == b.num
        && a.min == b.min
        && a.max == b.max
        && a.step == b.step
        && a.selected == b.selected
        && a.has_icon == b.has_icon
        && options(a) == options(b)
}

/// The rows of a group model, when it is the list the panel made.
fn row_list(group: &SettingGroup) -> Option<&VecModel<SettingRow>> {
    group.rows.as_any().downcast_ref::<VecModel<SettingRow>>()
}

/// Removes the rows that have folded away (and the groups left empty).
fn drop_leaving(window: &SettingsWindow) {
    let groups = window.get_groups();
    let Some(list) = groups.as_any().downcast_ref::<VecModel<SettingGroup>>() else {
        return;
    };
    for i in (0..list.row_count()).rev() {
        let Some(group) = list.row_data(i) else {
            continue;
        };
        if let Some(rows) = row_list(&group) {
            for j in (0..rows.row_count()).rev() {
                if rows.row_data(j).is_some_and(|r| r.leaving) {
                    rows.remove(j);
                }
            }
            if rows.row_count() == 0 {
                list.remove(i);
            }
        }
    }
}

/// Puts back, where they were, the rows of `old` that `new` no longer has, marked as leaving:
/// they fold away instead of vanishing.
fn keep_leaving(old: &[Vec<SettingRow>], new: &mut Vec<Vec<SettingRow>>) {
    let kept: std::collections::HashSet<SharedString> =
        new.iter().flatten().map(|r| r.id.clone()).collect();
    let find = |new: &[Vec<SettingRow>], id: &SharedString| {
        new.iter()
            .enumerate()
            .find_map(|(g, rows)| rows.iter().position(|r| r.id == *id).map(|i| (g, i)))
    };
    for (gi, group) in old.iter().enumerate() {
        for (ri, row) in group.iter().enumerate() {
            if kept.contains(&row.id) {
                continue;
            }
            let mut leaving = row.clone();
            leaving.leaving = true;
            leaving.fresh = false;
            // After the row above it (still there, or leaving too), else before the next one
            // that stays.
            let place = group[..ri]
                .iter()
                .rev()
                .find_map(|r| find(new, &r.id).map(|(g, i)| (g, i + 1)))
                .or_else(|| group[ri + 1..].iter().find_map(|r| find(new, &r.id)));
            match place {
                Some((g, i)) => new[g].insert(i, leaving),
                // The whole group goes: after the group of the rows above it.
                None => {
                    let at = old[..gi]
                        .iter()
                        .rev()
                        .flatten()
                        .find_map(|r| find(new, &r.id).map(|(g, _)| g + 1))
                        .unwrap_or(0);
                    new.insert(at.min(new.len()), vec![leaving]);
                }
            }
        }
    }
}

/// Shows `groups`. When the page keeps its shape (same groups, same rows) only the rows that
/// changed are updated, in place: their controls stay, so a toggle slides instead of being
/// redrawn in its new state, and nothing flickers. On the same page, rows that appear unfold
/// and rows that go fold away.
fn show_groups(window: &SettingsWindow, groups: Vec<Vec<SettingRow>>) {
    // Rows still folding away from the change before go at once.
    drop_leaving(window);
    let current = window.get_groups();
    let same_shape = current.row_count() == groups.len()
        && groups.iter().enumerate().all(|(i, rows)| {
            current.row_data(i).is_some_and(|g| {
                g.rows.row_count() == rows.len()
                    && rows
                        .iter()
                        .enumerate()
                        .all(|(j, r)| g.rows.row_data(j).is_some_and(|old| old.id == r.id))
            })
        });
    if same_shape {
        for (i, rows) in groups.into_iter().enumerate() {
            let Some(group) = current.row_data(i) else {
                continue;
            };
            for (j, row) in rows.into_iter().enumerate() {
                if group
                    .rows
                    .row_data(j)
                    .is_some_and(|old| !same_row(&old, &row))
                {
                    group.rows.set_row_data(j, row);
                }
            }
        }
        return;
    }
    let old: Vec<Vec<SettingRow>> = current.iter().map(|g| g.rows.iter().collect()).collect();
    let old_ids: std::collections::HashSet<SharedString> =
        old.iter().flatten().map(|r| r.id.clone()).collect();
    let same_page = groups.iter().flatten().any(|r| old_ids.contains(&r.id));
    let mut groups: Vec<Vec<SettingRow>> = groups
        .into_iter()
        .map(|mut rows| {
            for r in &mut rows {
                r.fresh = same_page && !old_ids.contains(&r.id);
            }
            rows
        })
        .collect();
    let folding = same_page && window.get_animated();
    if folding {
        keep_leaving(&old, &mut groups);
    }
    let model: Vec<SettingGroup> = groups
        .into_iter()
        .map(|rows| SettingGroup {
            rows: ModelRc::from(Rc::new(VecModel::from(rows))),
        })
        .collect();
    window.set_groups(ModelRc::from(Rc::new(VecModel::from(model))));
    if folding {
        // Once folded (a little after the animation), they go.
        let weak = window.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(260), move || {
            if let Some(w) = weak.upgrade() {
                drop_leaving(&w);
            }
        });
    }
}

/// Whether a shortcut starts a group of actions (captures, recording, the program).
const fn starts_group(action: ActionId) -> bool {
    matches!(
        action,
        ActionId::CaptureRegion | ActionId::RecordToggle | ActionId::OverlayToggle
    )
}

fn picture(icon: Option<&vixeeny_settings::Icon>) -> slint::Image {
    icon.map_or_else(slint::Image::default, |icon| {
        slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
            &icon.rgba,
            icon.width,
            icon.height,
        ))
    })
}

fn row_model(row: &Row, config: &Config) -> SettingRow {
    let value = row.value(config);
    let mut out = SettingRow {
        id: row.id.as_str().into(),
        label: row.label.as_str().into(),
        hint: row.hint.as_str().into(),
        info: row.info.as_str().into(),
        wide: 0,
        kind: 5,
        enabled: row.enabled(),
        on: false,
        text: SharedString::new(),
        num: 0,
        min: 0,
        max: 0,
        step: 1,
        options: ModelRc::default(),
        headings: ModelRc::default(),
        selected: -1,
        has_icon: row.icon.is_some(),
        fresh: false,
        leaving: false,
        icon: picture(row.icon.as_ref()),
    };
    match (&row.kind, value) {
        (Kind::Toggle, Value::Bool(b)) => {
            out.kind = 0;
            out.on = b;
        }
        (Kind::Choice(options), Value::Text(t)) => {
            out.kind = match row.picker {
                Picker::List => 1,
                Picker::Edge => 10,
                Picker::Corner => 11,
            };
            out.selected = options
                .iter()
                .position(|o| o.value == t)
                .map_or(-1, |i| i as i32);
            out.options = strings(options.iter().map(|o| o.label.clone()));
            // About 7.5 px a character at 14 px, plus the margins and the chevron.
            let longest = options
                .iter()
                .map(|o| o.label.chars().count())
                .max()
                .unwrap_or(0);
            out.wide = (longest as f32 * 7.5 + 60.0).min(440.0) as i32;
            if options.iter().any(|o| o.heading) {
                out.headings = ModelRc::from(Rc::new(VecModel::from(
                    options.iter().map(|o| o.heading).collect::<Vec<_>>(),
                )));
            }
        }
        (Kind::Meter, _) => out.kind = 6,
        (Kind::Expander, Value::Bool(open)) => {
            out.kind = 9;
            out.on = open;
        }
        (Kind::Number { min, max, step } | Kind::Slider { min, max, step }, Value::Int(n)) => {
            out.kind = if matches!(row.kind, Kind::Slider { .. }) {
                8
            } else {
                2
            };
            out.num = n as i32;
            out.min = *min as i32;
            out.max = *max as i32;
            out.step = *step as i32;
        }
        (Kind::Text, Value::Text(t)) => {
            out.kind = 3;
            out.text = t.into();
        }
        (Kind::Folder, Value::Text(t)) => {
            out.kind = 4;
            out.text = vixeeny_common::paths::display_dir(&t).into();
        }
        (_, Value::Text(t)) => out.text = t.into(),
        _ => {}
    }
    out
}

impl State {
    fn lang(&self) -> Lang {
        Lang::resolve(
            &self.config.borrow().general.language,
            self.os_locale.as_deref(),
        )
    }

    fn changed(&self) {
        (self.on_change.borrow())(&self.config.borrow());
    }
}

/// The title bar drawn by the window itself: moving, minimizing, maximizing, closing and
/// resizing go to the system through winit. Windows 11 still gives the frameless window its
/// shadow and rounded corners.
fn wire_chrome(window: &SettingsWindow) {
    window.on_minimize_window({
        let weak = window.as_weak();
        move || {
            if let Some(w) = weak.upgrade() {
                w.window().set_minimized(true);
            }
        }
    });
    window.on_toggle_maximized({
        let weak = window.as_weak();
        move || {
            if let Some(w) = weak.upgrade() {
                let maximized = !w.window().is_maximized();
                w.window().set_maximized(maximized);
                w.set_zoomed(maximized);
            }
        }
    });
    window.on_close_window({
        let weak = window.as_weak();
        move || {
            if let Some(w) = weak.upgrade() {
                w.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            }
        }
    });
    #[cfg(feature = "desktop")]
    {
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::window::ResizeDirection;
        window.on_drag_window({
            let weak = window.as_weak();
            move || {
                if let Some(w) = weak.upgrade() {
                    w.window().with_winit_window(|w| {
                        let _ = w.drag_window();
                    });
                }
            }
        });
        window.on_resize_window({
            let weak = window.as_weak();
            move |direction| {
                let direction = match direction {
                    0 => ResizeDirection::East,
                    1 => ResizeDirection::North,
                    2 => ResizeDirection::NorthEast,
                    3 => ResizeDirection::NorthWest,
                    4 => ResizeDirection::South,
                    5 => ResizeDirection::SouthEast,
                    6 => ResizeDirection::SouthWest,
                    _ => ResizeDirection::West,
                };
                if let Some(w) = weak.upgrade() {
                    w.window().with_winit_window(|w| {
                        let _ = w.drag_resize_window(direction);
                    });
                }
            }
        });
        // Maximized by the system too (double click, Win+Up, a drag to the top): follow it.
        let weak = window.as_weak();
        window.window().on_winit_window_event(move |_, event| {
            use slint::winit_030::EventResult;
            use slint::winit_030::winit::event::WindowEvent;
            if matches!(event, WindowEvent::Resized(_))
                && let Some(w) = weak.upgrade()
            {
                let maximized = w.window().is_maximized();
                w.set_zoomed(maximized);
            }
            EventResult::Propagate
        });
        theme::when_native(window, {
            let weak = window.as_weak();
            move |_| {
                use slint::winit_030::winit::platform::windows::{
                    CornerPreference, WindowExtWindows,
                };
                if let Some(w) = weak.upgrade() {
                    w.window().with_winit_window(|w| {
                        w.set_undecorated_shadow(true);
                        w.set_corner_preference(CornerPreference::Round);
                    });
                }
            }
        });
    }
}

impl SettingsPanel {
    pub fn new(
        config: Config,
        os_locale: Option<String>,
        version: &str,
        look: Look,
    ) -> Result<Self, slint::PlatformError> {
        let window = SettingsWindow::new()?;
        let lang = Lang::resolve(&config.general.language, os_locale.as_deref());
        let state = Rc::new(State {
            env: RefCell::new(Env::new(lang, version)),
            config: RefCell::new(config),
            os_locale,
            version: version.to_owned(),
            rows: RefCell::default(),
            probe: RefCell::new((None, false)),
            audio: RefCell::default(),
            machine: RefCell::default(),
            open: RefCell::default(),
            audio_source: RefCell::default(),
            display: Cell::default(),
            recording: Cell::new(None),
            chord: RefCell::default(),
            on_recording: RefCell::new(Box::new(|_| {})),
            on_change: RefCell::new(Box::new(|_| {})),
            on_browse: RefCell::new(Box::new(|_| None)),
            on_page_action: RefCell::new(Box::new(|_, _, _| {})),
            on_section: RefCell::new(Box::new(|_| {})),
            found: RefCell::default(),
            taken: RefCell::default(),
            metered: RefCell::default(),
            on_meter: RefCell::new(Box::new(|_| {})),
        });
        theme::apply(&window, look);
        wire_chrome(&window);
        let panel = Self { window, state };
        panel.wire();
        panel.relabel();
        panel.refresh();
        Ok(panel)
    }

    pub fn window(&self) -> &SettingsWindow {
        &self.window
    }

    /// The settings as they are now.
    pub fn config(&self) -> Config {
        self.state.config.borrow().clone()
    }

    /// Called with the new settings after every change.
    pub fn on_change(&self, f: impl Fn(&Config) + 'static) {
        *self.state.on_change.borrow_mut() = Box::new(f);
    }

    /// Asks the host to choose a folder (starting at the given one).
    pub fn on_browse(&self, f: impl Fn(&str) -> Option<String> + 'static) {
        *self.state.on_browse.borrow_mut() = Box::new(f);
    }

    /// A button of the hardware, integration, updates or about pages was pressed.
    pub fn on_page_action(&self, f: impl Fn(Section, &str, &str) + 'static) {
        *self.state.on_page_action.borrow_mut() = Box::new(f);
    }

    /// The user opened a section: the host fills the pages that show live data.
    pub fn on_section(&self, f: impl Fn(Section) + 'static) {
        *self.state.on_section.borrow_mut() = Box::new(f);
    }

    /// The programs, microphones and outputs the audio page offers (found by the host).
    /// What the monitors can show (HDR, refresh rate): the video page offers what fits.
    pub fn set_display(&self, display: Display) {
        self.state.display.set(display);
        self.rebuild_env();
        self.refresh();
    }

    /// The shortcuts the daemon could not register (their text as the settings file has them,
    /// any spelling): the shortcuts page marks them.
    pub fn set_taken_shortcuts(&self, taken: Vec<String>) {
        if *self.state.taken.borrow() == taken {
            return;
        }
        *self.state.taken.borrow_mut() = taken;
        if self.current_section() == Section::Shortcuts {
            self.refresh_shortcuts(&[]);
        }
    }

    /// Asks `read` for the shortcuts the daemon could not register now, and again every second
    /// while the window is open.
    pub fn watch_taken_shortcuts(&self, read: impl Fn() -> Vec<String> + 'static) {
        self.set_taken_shortcuts(read());
        let (weak, state) = (self.window.as_weak(), self.state.clone());
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(1),
            move || {
                if let Some(window) = weak.upgrade() {
                    SettingsPanel {
                        window,
                        state: state.clone(),
                    }
                    .set_taken_shortcuts(read());
                }
            },
        );
        // Runs as long as the window: the program ends with it.
        std::mem::forget(timer);
    }

    /// Called with the microphone to measure (`mic` or `mic:<id>`) while the audio page shows
    /// its level, and with `None` when it stops showing it. The host then measures it and
    /// reports through [`SettingsPanel::set_mic_level`].
    pub fn on_meter(&self, f: impl Fn(Option<String>) + 'static) {
        *self.state.on_meter.borrow_mut() = Box::new(f);
    }

    /// How loud the microphone is now, 0 to 1.
    pub fn set_mic_level(&self, level: f32) {
        self.window.set_mic_level(level);
    }

    pub fn set_audio(&self, audio: AudioDevices) {
        *self.state.audio.borrow_mut() = audio;
        self.rebuild_env();
        self.refresh();
    }

    /// Where the audio page's devices and programs come from: asked on another thread now,
    /// and again each time the page is opened (programs come and go).
    pub fn set_audio_source(&self, source: impl Fn() -> AudioDevices + Send + Sync + 'static) {
        *self.state.audio_source.borrow_mut() = Some(std::sync::Arc::new(source));
        self.load_audio();
    }

    fn load_audio(&self) {
        let Some(source) = self.state.audio_source.borrow().clone() else {
            return;
        };
        self.in_background("audio-list", move || source(), Self::set_audio);
    }

    /// What this computer is made of, for the general page: read once, on another thread.
    pub fn set_machine_source(&self, source: impl FnOnce() -> Machine + Send + 'static) {
        self.in_background("machine", source, Self::set_machine);
    }

    pub fn set_machine(&self, machine: Machine) {
        *self.state.machine.borrow_mut() = machine;
        self.rebuild_env();
        self.refresh();
    }

    /// Runs `make` on a thread of its own, then `apply` with its answer on this one.
    fn in_background<T: Send + 'static>(
        &self,
        name: &str,
        make: impl FnOnce() -> T + Send + 'static,
        apply: fn(&Self, T),
    ) {
        let slot: std::sync::Arc<std::sync::Mutex<Option<T>>> = std::sync::Arc::default();
        let filled = slot.clone();
        let started = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let value = make();
                if let Ok(mut s) = filled.lock() {
                    *s = Some(value);
                }
            });
        if started.is_err() {
            return;
        }
        // Looked at a few times a second until the answer is there, then not any more.
        fn poll<T: Send + 'static>(
            slot: std::sync::Arc<std::sync::Mutex<Option<T>>>,
            weak: slint::Weak<SettingsWindow>,
            state: Rc<State>,
            apply: fn(&SettingsPanel, T),
        ) {
            let ready = slot.lock().ok().and_then(|mut s| s.take());
            match (ready, weak.upgrade()) {
                (Some(value), Some(window)) => apply(&SettingsPanel { window, state }, value),
                (None, Some(_)) => {
                    slint::Timer::single_shot(std::time::Duration::from_millis(40), move || {
                        poll(slot, weak, state, apply);
                    });
                }
                (_, None) => {}
            }
        }
        poll(slot, self.window.as_weak(), self.state.clone(), apply);
    }

    /// The hardware probe: its result once it exists, and whether it is still running.
    pub fn set_probe(&self, probe: Option<ProbeResult>, detecting: bool) {
        *self.state.probe.borrow_mut() = (probe, detecting);
        self.rebuild_env();
        self.refresh();
    }

    /// Called with `true` when a shortcut starts being recorded and `false` when it stops: the
    /// host stops the global shortcuts meanwhile, or pressing one would run it.
    pub fn on_recording(&self, f: impl Fn(bool) + 'static) {
        *self.state.on_recording.borrow_mut() = Box::new(f);
    }

    /// Looks at `provider` every second while the hardware probe is running, and fills the
    /// video page once it has an answer. `provider` returns `(result, still running)`.
    pub fn watch_probe(&self, provider: impl Fn() -> (Option<ProbeResult>, bool) + 'static) {
        let (weak, state) = (self.window.as_weak(), self.state.clone());
        let timer = Rc::new(slint::Timer::default());
        let holder = timer.clone();
        let first = provider();
        let running = first.1;
        self.set_probe(first.0, first.1);
        if !running {
            return;
        }
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(900),
            move || {
                let _keep = &holder;
                let (probe, detecting) = provider();
                if let Some(window) = weak.upgrade() {
                    *state.probe.borrow_mut() = (probe, detecting);
                    let panel = SettingsPanel {
                        window,
                        state: state.clone(),
                    };
                    panel.rebuild_env();
                    panel.refresh();
                    if !detecting {
                        holder.stop();
                    }
                }
            },
        );
        // The timer lives as long as the closure above, which the timer itself keeps.
        std::mem::forget(timer);
    }

    pub fn select_section(&self, section: Section) {
        let index = Section::ALL.iter().position(|s| *s == section).unwrap_or(1);
        self.window.set_section(index as i32);
        self.refresh();
        (self.state.on_section.borrow())(section);
    }

    pub fn current_section(&self) -> Section {
        Section::ALL
            .get(self.window.get_section() as usize)
            .copied()
            .unwrap_or(Section::General)
    }

    pub fn set_lines(&self, lines: Vec<Line>) {
        self.window.set_lines(line_model(lines, None));
    }

    pub fn set_update(&self, view: &UpdateView) {
        show_update(&self.window, view);
    }

    /// A handle for other threads.
    pub fn handle(&self) -> PanelHandle {
        PanelHandle(self.window.as_weak())
    }

    /// A message under the page (an error, a hint).
    pub fn set_extra(&self, text: &str) {
        self.window.set_extra(text.into());
    }

    /// Translated texts for the current language; called again when the language changes.
    fn relabel(&self) {
        let lang = self.state.lang();
        self.rebuild_env();
        self.window.set_t(ui_texts(lang));
        self.window.set_section_titles(strings(
            Section::ALL.iter().map(|s| tr(s.title(), lang).to_owned()),
        ));
        self.window
            .set_version(format!("Version {}", self.state.version).into());
        self.window.set_tagline(tr(Key::AboutTagline, lang).into());
        self.window
            .set_search_placeholder(tr(Key::UiSearch, lang).into());
        self.window
            .set_search_title(tr(Key::UiSearchTitle, lang).into());
    }

    /// The model's view of this machine: language, hardware probe, audio devices.
    fn rebuild_env(&self) {
        let state = &self.state;
        let (probe, detecting) = state.probe.borrow().clone();
        *state.env.borrow_mut() = Env::new(state.lang(), &state.version)
            .with_probe(probe, detecting)
            .with_audio(state.audio.borrow().clone())
            .with_machine(state.machine.borrow().clone())
            .with_open(state.open.borrow().clone())
            .with_display(state.display.get());
    }

    /// Rebuilds what the current page shows from the settings.
    pub fn refresh(&self) {
        let state = &self.state;
        let section = self.current_section();
        let config = state.config.borrow();
        let env = state.env.borrow();
        if section.is_rows() {
            let built = rows(section, &env, &config);
            show_groups(&self.window, grouped(&built, &config));
            // The level of the microphone, while its meter is on screen.
            let metered = built
                .iter()
                .any(|r| matches!(r.kind, Kind::Meter))
                .then(|| {
                    config
                        .video
                        .audio
                        .sources
                        .iter()
                        .find(|s| *s == "mic" || s.starts_with("mic:"))
                        .cloned()
                })
                .flatten();
            *state.rows.borrow_mut() = built;
            self.meter(metered);
        } else {
            self.meter(None);
            self.window.set_groups(ModelRc::default());
            state.rows.borrow_mut().clear();
        }
        let overview = if section == Section::General {
            vixeeny_settings::machine::overview(&env)
        } else {
            Vec::new()
        };
        self.window.set_pc_lines(pc_lines(&overview));
        let notice = if section == Section::Video {
            video_problems(&config, (1920, 1080), env.lang).join("\n")
        } else {
            String::new()
        };
        let heading = if notice.is_empty() {
            String::new()
        } else {
            format!("{}\n{notice}", tr(Key::SetProblems, env.lang))
        };
        self.window.set_notice(heading.into());
        if section == Section::Shortcuts {
            drop(config);
            drop(env);
            self.refresh_shortcuts(&[]);
        }
    }

    /// Starts or stops the measure of the microphone when what the page shows changes.
    fn meter(&self, metered: Option<String>) {
        if *self.state.metered.borrow() == metered {
            return;
        }
        if metered.is_none() {
            self.window.set_mic_level(0.0);
        }
        *self.state.metered.borrow_mut() = metered.clone();
        (self.state.on_meter.borrow())(metered);
    }

    fn refresh_shortcuts(&self, errors: &[(usize, String)]) {
        let config = self.state.config.borrow();
        let taken = self.state.taken.borrow();
        let lang = self.state.lang();
        let table: Vec<ShortcutRow> = shortcuts::listed(&config)
            .into_iter()
            .enumerate()
            .map(|(i, (action, slots))| {
                // Kept by Windows, or refused by the OS when the daemon registered it.
                let windows = |text: &str| shortcuts::kept_by_windows(text);
                let other = |text: &str| taken.iter().any(|t| shortcuts::same(t, text));
                let keys: Vec<ShortcutKey> = slots
                    .iter()
                    .enumerate()
                    .map(|(slot, text)| ShortcutKey {
                        text: text.as_str().into(),
                        shown: shortcut_label(text, lang).into(),
                        slot: slot as i32,
                        taken: windows(text) || other(text),
                    })
                    .collect();
                let unusable = if slots.iter().any(|t| windows(t)) {
                    tr(Key::ShortcutWindows, lang)
                } else if slots.iter().any(|t| other(t)) {
                    tr(Key::ShortcutTaken, lang)
                } else {
                    ""
                };
                ShortcutRow {
                    group: starts_group(action),
                    label: action_label(action, lang).into(),
                    keys: ModelRc::from(Rc::new(VecModel::from(keys))),
                    error: errors
                        .iter()
                        .find(|(row, _)| *row == i)
                        .map_or_else(|| unusable.into(), |(_, e)| e.as_str().into()),
                }
            })
            .collect();
        self.window
            .set_shortcuts(ModelRc::from(Rc::new(VecModel::from(table))));
    }

    /// Writes `text` into a slot of a shortcut (empty clears it); errors show under the row.
    fn set_shortcut(&self, row: usize, slot: usize, text: &str) {
        let state = &self.state;
        let lang = state.lang();
        let listed = shortcuts::listed(&state.config.borrow());
        let Some(action) = listed.get(row).map(|(action, _)| *action) else {
            return;
        };
        let result = shortcuts::set(&mut state.config.borrow_mut(), action, slot, text);
        let errors = match result {
            Ok(()) => {
                state.changed();
                Vec::new()
            }
            Err(refusal) => vec![(
                row,
                match refusal {
                    Refusal::Invalid(message) => message,
                    Refusal::Duplicate => tr(Key::ShortcutDuplicate, lang).to_owned(),
                    Refusal::Conflict(other) => tr(Key::ShortcutConflict, lang)
                        .replace("{action}", action_label(other, lang)),
                },
            )],
        };
        self.refresh_shortcuts(&errors);
    }

    /// Starts (or stops, with `None`) recording the shortcut of `(row, slot)`.
    fn record(&self, target: Option<(usize, usize)>) {
        let was = self.state.recording.replace(target).is_some();
        *self.state.chord.borrow_mut() = Chord::default();
        self.window.set_recording_keys(SharedString::new());
        self.window
            .set_recording_row(target.map_or(-1, |(row, _)| row as i32));
        self.window
            .set_recording_slot(target.map_or(-1, |(_, slot)| slot as i32));
        if was != target.is_some() {
            (self.state.on_recording.borrow())(target.is_some());
        }
    }

    /// What the user pressed while a shortcut is being recorded (the combination, or the slot
    /// emptied). Returns whether a shortcut was being recorded.
    pub fn captured(&self, pressed: Captured) -> bool {
        let Some((row, slot)) = self.state.recording.get() else {
            return false;
        };
        match pressed {
            Captured::Cancel => {}
            Captured::Clear => self.set_shortcut(row, slot, ""),
            Captured::Combination(text) => self.set_shortcut(row, slot, &text),
        }
        self.record(None);
        true
    }

    /// A key went down (`down`) or up while a shortcut may be recorded; `code` is its
    /// `KeyboardEvent.code` name. Returns whether the key was taken.
    pub fn key(&self, code: &str, down: bool) -> bool {
        if self.state.recording.get().is_none() {
            return false;
        }
        let step = {
            let mut chord = self.state.chord.borrow_mut();
            if down {
                chord.press(code)
            } else {
                chord.release(code)
            }
        };
        match step {
            ChordStep::Held(text) => {
                let label = shortcut_label(&text, self.state.lang());
                self.window.set_recording_keys(label.into());
            }
            ChordStep::Done(pressed) => {
                self.captured(pressed);
            }
        }
        true
    }

    /// Hooks the keyboard of the window so that a recorded shortcut is read from the keys
    /// themselves. Needs the real (winit) backend.
    #[cfg(feature = "desktop")]
    pub fn capture_keys(&self) {
        use slint::winit_030::winit::event::{ElementState, WindowEvent};
        use slint::winit_030::winit::keyboard::PhysicalKey;
        use slint::winit_030::{EventResult, WinitWindowAccessor};
        let (weak, state) = (self.window.as_weak(), self.state.clone());
        self.window.window().on_winit_window_event(move |_, event| {
            let panel = || {
                weak.upgrade().map(|window| SettingsPanel {
                    window,
                    state: state.clone(),
                })
            };
            match event {
                // Elsewhere, the keys go to another program: the recording stops.
                WindowEvent::Focused(false) => {
                    if let Some(p) = panel() {
                        p.captured(Captured::Cancel);
                    }
                }
                WindowEvent::KeyboardInput { event, .. } if state.recording.get().is_some() => {
                    if let (PhysicalKey::Code(code), false, Some(p)) =
                        (event.physical_key, event.repeat, panel())
                    {
                        p.key(&format!("{code:?}"), event.state == ElementState::Pressed);
                    }
                    return EventResult::PreventDefault;
                }
                _ => {}
            }
            EventResult::Propagate
        });
    }

    /// Lists the settings that answer `query` (empty: back to the page).
    fn search(&self, query: &str) {
        let state = &self.state;
        let lang = state.lang();
        let found = if query.trim().is_empty() {
            Vec::new()
        } else {
            let config = state.config.borrow();
            let mut found = search(query, &state.env.borrow(), &config);
            // The actions of the shortcuts page.
            for (action, _) in shortcuts::listed(&config) {
                let label = action_label(action, lang);
                if vixeeny_settings::matches(query, label) {
                    found.push(Found {
                        section: Section::Shortcuts,
                        id: String::new(),
                        label: label.to_owned(),
                    });
                }
            }
            found
        };
        let hits: Vec<SearchHit> = found
            .iter()
            .map(|f| SearchHit {
                label: f.label.as_str().into(),
                page: tr(f.section.title(), lang).into(),
                section: Section::ALL
                    .iter()
                    .position(|s| *s == f.section)
                    .unwrap_or(0) as i32,
            })
            .collect();
        self.window
            .set_search_hits(ModelRc::from(Rc::new(VecModel::from(hits))));
        self.window.set_search_none(
            tr(Key::UiSearchNone, lang)
                .replace("{query}", query.trim())
                .into(),
        );
        *state.found.borrow_mut() = found;
    }

    /// Leaves the search for the page of `found`, the setting outlined and in view.
    fn go_to(&self, found: &Found) {
        let state = &self.state;
        self.window.set_search_text(SharedString::new());
        state.found.borrow_mut().clear();
        // A folded setting: its section opens.
        let shown = || state.rows.borrow().iter().any(|r| r.id == found.id);
        self.select_section(found.section);
        if found.section == Section::Video && !found.id.is_empty() && !shown() {
            state.open.borrow_mut().insert("expert_video".into());
            self.rebuild_env();
            self.refresh();
        }
        if found.id.is_empty() {
            return;
        }
        let id: SharedString = found.id.as_str().into();
        self.window.set_highlight_id(id.clone());
        // The outline fades after a moment.
        let weak = self.window.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(1600), move || {
            if let Some(w) = weak.upgrade()
                && w.get_highlight_id() == id
            {
                w.set_highlight_id(SharedString::new());
            }
        });
    }

    fn wire(&self) {
        let w = &self.window;
        let weak = w.as_weak();
        let panel = |weak: &slint::Weak<SettingsWindow>, state: &Rc<State>| {
            weak.upgrade().map(|window| SettingsPanel {
                window,
                state: state.clone(),
            })
        };

        w.on_section_changed({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |i| {
                if let Some(p) = panel(&weak, &state) {
                    p.refresh();
                    if let Some(section) = Section::ALL.get(i as usize) {
                        if *section == Section::Audio {
                            p.load_audio();
                        }
                        (state.on_section.borrow())(*section);
                    }
                }
            }
        });
        // Rows: write, tell the host, redraw (other rows may change state).
        let edit = {
            let (weak, state) = (weak.clone(), self.state.clone());
            Rc::new(move |id: &str, value: Value| {
                let Some(p) = panel(&weak, &state) else {
                    return;
                };
                // An expander opens or closes rows: the window's state, not a setting.
                let expander = state
                    .rows
                    .borrow()
                    .iter()
                    .any(|r| r.id == id && matches!(r.kind, Kind::Expander));
                if expander {
                    {
                        let mut open = state.open.borrow_mut();
                        if !open.remove(id) {
                            open.insert(id.to_owned());
                        }
                    }
                    p.rebuild_env();
                    p.refresh();
                    return;
                }
                let result = {
                    let rows = state.rows.borrow();
                    let mut config = state.config.borrow_mut();
                    rows.iter().find(|r| r.id == id).map(|r| {
                        // A text field reports again when it loses the focus: not a change.
                        if r.value(&config) == value {
                            Err(vixeeny_settings::Invalid)
                        } else {
                            r.apply(&mut config, value)
                        }
                    })
                };
                if matches!(result, Some(Ok(()))) {
                    state.changed();
                    if id == "language" {
                        p.relabel();
                    }
                }
                p.refresh();
            })
        };
        w.on_row_toggled({
            let edit = edit.clone();
            move |id, on| edit(&id, Value::Bool(on))
        });
        w.on_row_number({
            let edit = edit.clone();
            move |id, n| edit(&id, Value::Int(i64::from(n)))
        });
        w.on_row_text({
            let edit = edit.clone();
            move |id, text| edit(&id, Value::Text(text.to_string()))
        });
        w.on_row_chosen({
            let (edit, state) = (edit.clone(), self.state.clone());
            move |id, index| {
                let value = state
                    .rows
                    .borrow()
                    .iter()
                    .find(|r| r.id == id.as_str())
                    .and_then(|r| match &r.kind {
                        Kind::Choice(options) => {
                            options.get(index as usize).map(|o| o.value.clone())
                        }
                        _ => None,
                    });
                if let Some(value) = value {
                    edit(&id, Value::Text(value));
                }
            }
        });
        w.on_row_browse({
            let (edit, state) = (edit, self.state.clone());
            move |id| {
                let current = state
                    .rows
                    .borrow()
                    .iter()
                    .find(|r| r.id == id.as_str())
                    .map(|r| r.value(&state.config.borrow()));
                let start = match current {
                    Some(Value::Text(t)) => t,
                    _ => String::new(),
                };
                let picked = (state.on_browse.borrow())(&start);
                if let Some(folder) = picked {
                    edit(&id, Value::Text(folder));
                }
            }
        });

        w.on_search_edited({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |text| {
                if let Some(p) = panel(&weak, &state) {
                    p.search(&text);
                }
            }
        });
        w.on_search_picked({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |i| {
                let found = state.found.borrow().get(i.max(0) as usize).cloned();
                if let (Some(p), Some(found)) = (panel(&weak, &state), found) {
                    p.go_to(&found);
                }
            }
        });
        w.on_shortcut_record({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |row, slot| {
                if let Some(p) = panel(&weak, &state) {
                    p.record(Some((row.max(0) as usize, slot.max(0) as usize)));
                }
            }
        });
        w.on_shortcut_clear({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |row, slot| {
                if let Some(p) = panel(&weak, &state) {
                    p.record(None);
                    p.set_shortcut(row.max(0) as usize, slot.max(0) as usize, "");
                }
            }
        });

        // The button that opens a folder row's folder: the host opens it.
        w.on_row_action({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |id, action| {
                let Some(p) = panel(&weak, &state) else {
                    return;
                };
                let folder = state
                    .rows
                    .borrow()
                    .iter()
                    .find(|r| r.id == id.as_str())
                    .map(|r| r.value(&state.config.borrow()));
                if let (Some(Value::Text(folder)), "open") = (folder, action.as_str()) {
                    (state.on_page_action.borrow())(p.current_section(), "open-folder", &folder);
                }
            }
        });
        w.on_page_action({
            let (weak, state) = (weak, self.state.clone());
            move |action, arg| {
                let Some(p) = panel(&weak, &state) else {
                    return;
                };
                let section = p.current_section();
                (state.on_page_action.borrow())(section, &action, &arg);
            }
        });
    }
}

/// A shortcut as the field shows it: `Ctrl+Shift+KeyR` → `CTRL + SHIFT + R`.
pub fn shortcut_label(text: &str, lang: Lang) -> String {
    key_names(text, lang).join(" + ").to_uppercase()
}

/// The keys of a shortcut as the user reads them: `Ctrl+Shift+KeyR` → Ctrl, Shift, R.
pub fn key_names(text: &str, lang: Lang) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('+')
        .map(|part| {
            let name = part
                .strip_prefix("Key")
                .or_else(|| part.strip_prefix("Digit"))
                .unwrap_or(part);
            match name {
                "PrintScreen" if lang == Lang::Fr => "Impr. écran".to_owned(),
                "PrintScreen" => "Print Screen".to_owned(),
                other => other.to_owned(),
            }
        })
        .collect()
}
