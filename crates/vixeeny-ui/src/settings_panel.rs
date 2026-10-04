// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings window (plan 5.13). It owns a copy of the [`Config`], applies every change the
//! moment it is made (the host is told through [`SettingsPanel::on_change`]), and leaves what
//! needs the OS (folders to browse, the gallery's files, the hardware list) to the host.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedPixelBuffer, SharedString, VecModel};
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::ActionId;
use vixeeny_encode::probe::ProbeResult;
use vixeeny_settings::shortcuts::{self, Refusal};
use vixeeny_settings::{
    AudioDevices, Env, Kind, Row, Section, Value, profiles, reset, rows, video_problems,
};

use crate::theme::{self, Look};
use crate::{
    GalleryItem, LineItem, SettingGroup, SettingRow, SettingsWindow, ShortcutKey, ShortcutRow,
    UiTexts,
};

/// One tile of the gallery.
#[derive(Debug, Clone, Default)]
pub struct GalleryEntry {
    pub name: String,
    /// Date and size, as text.
    pub detail: String,
    pub video: bool,
    /// RGBA thumbnail, `(width, height, bytes)`.
    pub thumb: Option<(u32, u32, Vec<u8>)>,
}

/// One line of the about page.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub text: String,
    pub detail: String,
    pub strong: bool,
}

/// Where the update stands, for the card of the about page.
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

/// What the gallery asks of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GalleryRequest {
    Select(usize),
    /// Double click: open this tile.
    Open(usize),
    /// `open`, `folder`, `copy`, `convert` or `delete`, on the selected tile.
    Action(String),
    /// Kind (0 all, 1 images, 2 videos) and application text.
    Filter(i32, String),
}

type ChangeFn = Box<dyn Fn(&Config)>;
type BrowseFn = Box<dyn Fn(&str) -> Option<String>>;
type PageFn = Box<dyn Fn(Section, &str, &str)>;

struct State {
    config: RefCell<Config>,
    env: RefCell<Env>,
    os_locale: Option<String>,
    version: String,
    rows: RefCell<Vec<Row>>,
    /// The hardware probe, once known, and whether it is still running.
    probe: RefCell<(Option<ProbeResult>, bool)>,
    audio: RefCell<AudioDevices>,
    /// The shortcut being recorded: `(row, slot)`.
    recording: Cell<Option<(usize, usize)>>,
    on_recording: RefCell<Box<dyn Fn(bool)>>,
    on_change: RefCell<ChangeFn>,
    on_browse: RefCell<BrowseFn>,
    on_page_action: RefCell<PageFn>,
    on_gallery: RefCell<Box<dyn Fn(GalleryRequest)>>,
    on_section: RefCell<Box<dyn Fn(Section)>>,
}

/// A way to update the window from another thread (a detection that takes a while, thumbnails
/// that arrive one by one).
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

    /// Closes the window (the program then ends).
    pub fn close(&self) {
        let _ = self.0.upgrade_in_event_loop(|w| {
            let _ = w.hide();
        });
    }

    pub fn set_gallery(&self, entries: Vec<GalleryEntry>, selected: Option<usize>) {
        let _ = self.0.upgrade_in_event_loop(move |w| {
            w.set_gallery_selected(selected.map_or(-1, |i| i as i32));
            w.set_gallery(gallery_model(entries, selected));
        });
    }

    pub fn set_gallery_detail(&self, text: String) {
        let _ = self
            .0
            .upgrade_in_event_loop(move |w| w.set_gallery_detail(text.into()));
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
            strong: l.strong,
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(items)))
}

fn gallery_model(entries: Vec<GalleryEntry>, selected: Option<usize>) -> ModelRc<GalleryItem> {
    let items: Vec<GalleryItem> = entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            let thumb = e.thumb.and_then(|(w, h, rgba)| {
                (rgba.len() == w as usize * h as usize * 4).then(|| {
                    let mut buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                    buffer.make_mut_bytes().copy_from_slice(&rgba);
                    slint::Image::from_rgba8(buffer)
                })
            });
            GalleryItem {
                name: e.name.into(),
                detail: e.detail.into(),
                has_thumb: thumb.is_some(),
                thumb: thumb.unwrap_or_default(),
                video: e.video,
                selected: selected == Some(i),
            }
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(items)))
}

pub struct SettingsPanel {
    window: SettingsWindow,
    state: Rc<State>,
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
        reset: t(Key::SetReset),
        new_label: t(Key::UiNew),
        duplicate: t(Key::UiDuplicate),
        rename: t(Key::UiRename),
        delete: t(Key::UiDelete),
        use_label: t(Key::UiUse),
        open: t(Key::UiOpen),
        open_folder: t(Key::UiOpenFolder),
        copy: t(Key::UiCopy),
        all: t(Key::UiAll),
        images: t(Key::UiImages),
        videos: t(Key::UiVideos),
        app_filter: t(Key::UiAppFilter),
        redetect: t(Key::UiRedetect),
        check_now: t(Key::SetCheckNow),
        update_now: t(Key::UpdateNow),
        github: t(Key::UiGithub),
        logs: t(Key::UiLogs),
        copy_info: t(Key::UiCopyInfo),
        empty: t(Key::UiGalleryEmpty),
        name_hint: t(Key::UiNameHint),
        choose: t(Key::WizBrowse),
        press_keys: t(Key::UiPressKeys),
        keys_help: t(Key::UiKeysHelp),
        add_shortcut: t(Key::UiAddShortcut),
        details: t(Key::GrpLinks),
        restart: t(Key::UpdateRestart),
        retry: t(Key::UiRetry),
        profile_new: t(Key::ProfileNew),
        profile_duplicate: t(Key::ProfileDuplicate),
        profile_rename: t(Key::ProfileRename),
        profile_delete: t(Key::ProfileDelete),
        ok: t(Key::UiOk),
        cancel: t(Key::UiCancel),
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
            ActionId::OcrRegion => Key::ActOcr,
            ActionId::RecordToggle => Key::ActRecordToggle,
            ActionId::RecordPause => Key::ActRecordPause,
            ActionId::ReplayToggle => Key::ActReplayToggle,
            ActionId::ReplaySave => Key::ActReplaySave,
            ActionId::OverlayToggle => Key::ActOverlay,
            ActionId::OpenSettings => Key::ActSettings,
        },
        lang,
    )
}

/// The rows as groups: each header starts one.
fn group_model(rows: &[Row], config: &Config) -> ModelRc<SettingGroup> {
    let mut groups: Vec<(String, Vec<SettingRow>)> = Vec::new();
    for row in rows {
        if matches!(row.kind, Kind::Header) {
            groups.push((row.label.clone(), Vec::new()));
            continue;
        }
        if groups.is_empty() {
            groups.push((String::new(), Vec::new()));
        }
        if let Some((_, list)) = groups.last_mut() {
            list.push(row_model(row, config));
        }
    }
    let model: Vec<SettingGroup> = groups
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(title, rows)| SettingGroup {
            title: title.into(),
            rows: ModelRc::from(Rc::new(VecModel::from(rows))),
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(model)))
}

/// The group a shortcut belongs to, when it is the first of it.
fn shortcut_group(action: ActionId) -> Option<Key> {
    match action {
        ActionId::CaptureRegion => Some(Key::SecCapture),
        ActionId::RecordToggle => Some(Key::GrpRecording),
        ActionId::OverlayToggle => Some(Key::GrpApplication),
        _ => None,
    }
}

fn row_model(row: &Row, config: &Config) -> SettingRow {
    let value = row.value(config);
    let mut out = SettingRow {
        id: row.id.as_str().into(),
        label: row.label.as_str().into(),
        hint: row.hint.as_str().into(),
        kind: 5,
        enabled: row.enabled(config),
        on: false,
        text: SharedString::new(),
        num: 0,
        min: 0,
        max: 0,
        step: 1,
        options: ModelRc::default(),
        selected: -1,
    };
    match (&row.kind, value) {
        (Kind::Toggle, Value::Bool(b)) => {
            out.kind = 0;
            out.on = b;
        }
        (Kind::Header, _) => out.kind = 6,
        (Kind::Profile(names), Value::Text(t)) => {
            out.kind = 9;
            out.selected = names.iter().position(|n| *n == t).map_or(-1, |i| i as i32);
            out.options = strings(names.iter().cloned());
            out.text = t.into();
        }
        (Kind::Choice(options) | Kind::Segmented(options), Value::Text(t)) => {
            out.kind = if matches!(row.kind, Kind::Segmented(_)) {
                7
            } else {
                1
            };
            out.selected = options
                .iter()
                .position(|o| o.value == t)
                .map_or(-1, |i| i as i32);
            out.options = strings(options.iter().map(|o| o.label.clone()));
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
            recording: Cell::new(None),
            on_recording: RefCell::new(Box::new(|_| {})),
            on_change: RefCell::new(Box::new(|_| {})),
            on_browse: RefCell::new(Box::new(|_| None)),
            on_page_action: RefCell::new(Box::new(|_, _, _| {})),
            on_gallery: RefCell::new(Box::new(|_| {})),
            on_section: RefCell::new(Box::new(|_| {})),
        });
        theme::apply(&window, look);
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

    pub fn on_gallery(&self, f: impl Fn(GalleryRequest) + 'static) {
        *self.state.on_gallery.borrow_mut() = Box::new(f);
    }

    /// The user opened a section: the host fills the pages that show live data.
    pub fn on_section(&self, f: impl Fn(Section) + 'static) {
        *self.state.on_section.borrow_mut() = Box::new(f);
    }

    /// The programs, microphones and outputs the audio page offers (found by the host).
    pub fn set_audio(&self, audio: AudioDevices) {
        *self.state.audio.borrow_mut() = audio;
        self.rebuild_env();
        self.refresh();
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

    pub fn set_gallery(&self, entries: Vec<GalleryEntry>, selected: Option<usize>) {
        self.window.set_gallery(gallery_model(entries, selected));
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
    }

    /// The model's view of this machine: language, hardware probe, audio devices.
    fn rebuild_env(&self) {
        let state = &self.state;
        let (probe, detecting) = state.probe.borrow().clone();
        *state.env.borrow_mut() = Env::new(state.lang(), &state.version)
            .with_probe(probe, detecting)
            .with_audio(state.audio.borrow().clone());
    }

    /// Rebuilds what the current page shows from the settings.
    pub fn refresh(&self) {
        let state = &self.state;
        let section = self.current_section();
        let config = state.config.borrow();
        let env = state.env.borrow();
        self.window.set_can_reset(section.can_reset());
        if section.is_rows() {
            let built = rows(section, &env, &config);
            self.window.set_groups(group_model(&built, &config));
            *state.rows.borrow_mut() = built;
        } else {
            self.window.set_groups(ModelRc::default());
            state.rows.borrow_mut().clear();
        }
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

    fn refresh_shortcuts(&self, errors: &[(usize, String)]) {
        let config = self.state.config.borrow();
        let lang = self.state.lang();
        let table: Vec<ShortcutRow> = shortcuts::listed(&config)
            .into_iter()
            .enumerate()
            .map(|(i, (action, slots))| {
                let keys: Vec<ShortcutKey> = slots
                    .iter()
                    .enumerate()
                    .filter(|(_, text)| !text.is_empty())
                    .map(|(slot, text)| ShortcutKey {
                        text: text.as_str().into(),
                        slot: slot as i32,
                    })
                    .collect();
                ShortcutRow {
                    group: shortcut_group(action)
                        .map_or_else(SharedString::new, |k| tr(k, lang).into()),
                    label: action_label(action, lang).into(),
                    free_slot: slots
                        .iter()
                        .position(String::is_empty)
                        .map_or(-1, |slot| slot as i32),
                    keys: ModelRc::from(Rc::new(VecModel::from(keys))),
                    error: errors
                        .iter()
                        .find(|(row, _)| *row == i)
                        .map_or_else(SharedString::new, |(_, e)| e.as_str().into()),
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
        self.window
            .set_recording_row(target.map_or(-1, |(row, _)| row as i32));
        self.window
            .set_recording_slot(target.map_or(-1, |(_, slot)| slot as i32));
        if was != target.is_some() {
            (self.state.on_recording.borrow())(target.is_some());
        }
    }

    /// What the user pressed while a shortcut is being recorded (`Some(text)`: the combination,
    /// empty to clear). Returns whether the key was consumed.
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

    /// Hooks the keyboard of the window so that a recorded shortcut is read from the keys
    /// themselves. Needs the real (winit) backend.
    #[cfg(feature = "desktop")]
    pub fn capture_keys(&self) {
        use slint::winit_030::winit::event::{ElementState, WindowEvent};
        use slint::winit_030::winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
        use slint::winit_030::{EventResult, WinitWindowAccessor};
        let (weak, state) = (self.window.as_weak(), self.state.clone());
        let modifiers = Rc::new(Cell::new(ModifiersState::empty()));
        self.window.window().on_winit_window_event(move |_, event| {
            match event {
                WindowEvent::ModifiersChanged(m) => modifiers.set(m.state()),
                WindowEvent::KeyboardInput { event, .. }
                    if state.recording.get().is_some()
                        && event.state == ElementState::Pressed
                        && !event.repeat =>
                {
                    let PhysicalKey::Code(code) = event.physical_key else {
                        return EventResult::PreventDefault;
                    };
                    let m = modifiers.get();
                    let pressed = match code {
                        KeyCode::Escape => Some(Captured::Cancel),
                        KeyCode::Backspace if m.is_empty() => Some(Captured::Clear),
                        // A modifier alone is not a shortcut: wait for the key.
                        KeyCode::ControlLeft
                        | KeyCode::ControlRight
                        | KeyCode::ShiftLeft
                        | KeyCode::ShiftRight
                        | KeyCode::AltLeft
                        | KeyCode::AltRight
                        | KeyCode::SuperLeft
                        | KeyCode::SuperRight => None,
                        other => {
                            let mut text = String::new();
                            for (on, name) in [
                                (m.control_key(), "Ctrl+"),
                                (m.alt_key(), "Alt+"),
                                (m.shift_key(), "Shift+"),
                                (m.super_key(), "Win+"),
                            ] {
                                if on {
                                    text.push_str(name);
                                }
                            }
                            text.push_str(&format!("{other:?}"));
                            Some(Captured::Combination(text))
                        }
                    };
                    if let (Some(pressed), Some(window)) = (pressed, weak.upgrade()) {
                        let panel = SettingsPanel {
                            window,
                            state: state.clone(),
                        };
                        panel.captured(pressed);
                    }
                    return EventResult::PreventDefault;
                }
                _ => {}
            }
            EventResult::Propagate
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
                        (state.on_section.borrow())(*section);
                    }
                }
            }
        });
        w.on_reset({
            let (weak, state) = (weak.clone(), self.state.clone());
            move || {
                let Some(p) = panel(&weak, &state) else {
                    return;
                };
                let section = p.current_section();
                {
                    let env = state.env.borrow();
                    reset(section, &env, &mut state.config.borrow_mut());
                }
                state.changed();
                p.relabel();
                p.refresh();
            }
        });

        // Rows: write, tell the host, redraw (other rows may change state).
        let edit = {
            let (weak, state) = (weak.clone(), self.state.clone());
            Rc::new(move |id: &str, value: Value| {
                let Some(p) = panel(&weak, &state) else {
                    return;
                };
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
                        Kind::Choice(options) | Kind::Segmented(options) => {
                            options.get(index as usize).map(|o| o.value.clone())
                        }
                        Kind::Profile(names) => names.get(index as usize).cloned(),
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

        w.on_gallery_select({
            let state = self.state.clone();
            move |i| (state.on_gallery.borrow())(GalleryRequest::Select(i.max(0) as usize))
        });
        w.on_gallery_open({
            let state = self.state.clone();
            move |i| (state.on_gallery.borrow())(GalleryRequest::Open(i.max(0) as usize))
        });
        w.on_gallery_action({
            let state = self.state.clone();
            move |action| (state.on_gallery.borrow())(GalleryRequest::Action(action.to_string()))
        });
        w.on_gallery_filter({
            let state = self.state.clone();
            move |kind, app| {
                (state.on_gallery.borrow())(GalleryRequest::Filter(kind, app.to_string()));
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

        w.on_row_action({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |_, action, arg| {
                if let Some(p) = panel(&weak, &state) {
                    p.profile_action(&action, &arg);
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

    /// The buttons of the profile row: a new profile, a copy, a new name, or deleting it.
    fn profile_action(&self, action: &str, arg: &str) {
        let state = &self.state;
        let lang = state.lang();
        let result = {
            let mut config = state.config.borrow_mut();
            profiles::materialize(&mut config);
            let current = config.video.profile.clone();
            match action {
                "profile-new" => {
                    let name = profiles::free_name(&config, tr(Key::ProfileNewName, lang));
                    profiles::create(&mut config, &name)
                }
                "profile-duplicate" => {
                    let base = tr(Key::ProfileCopyName, lang).replace("{name}", &current);
                    let name = profiles::free_name(&config, &base);
                    profiles::duplicate(&mut config, &current, &name)
                }
                "profile-rename" => profiles::rename(&mut config, &current, arg),
                "profile-delete" => profiles::delete(&mut config, &current),
                _ => Ok(()),
            }
        };
        let notice = match result {
            Ok(()) => {
                state.changed();
                String::new()
            }
            Err(e) => tr(
                match e {
                    profiles::ProfileError::EmptyName | profiles::ProfileError::NotFound => {
                        Key::ProfileEmpty
                    }
                    profiles::ProfileError::Taken => Key::ProfileTaken,
                    profiles::ProfileError::LastOne => Key::ProfileLast,
                },
                lang,
            )
            .to_owned(),
        };
        self.refresh();
        if !notice.is_empty() {
            self.window.set_notice(notice.into());
        }
    }
}
