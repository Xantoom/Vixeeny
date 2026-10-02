// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings window (plan 5.13). It owns a copy of the [`Config`], applies every change the
//! moment it is made (the host is told through [`SettingsPanel::on_change`]), and leaves what
//! needs the OS (folders to browse, the gallery's files, the hardware list) to the host.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedPixelBuffer, SharedString, VecModel};
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::ActionId;
use vixeeny_settings::shortcuts::{self, Refusal};
use vixeeny_settings::{Env, Kind, Row, Section, Value, profiles, reset, rows, video_problems};

use crate::{GalleryItem, LineItem, SettingRow, SettingsWindow, ShortcutRow, UiTexts};

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

/// One line of a page (hardware, integration, about, profiles).
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub text: String,
    pub detail: String,
    pub strong: bool,
}

/// What the gallery asks of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GalleryRequest {
    Select(usize),
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
    selected_profile: RefCell<usize>,
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

    pub fn set_gallery(&self, entries: Vec<GalleryEntry>, selected: Option<usize>) {
        let _ = self
            .0
            .upgrade_in_event_loop(move |w| w.set_gallery(gallery_model(entries, selected)));
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
        convert: t(Key::UiConvert),
        all: t(Key::UiAll),
        images: t(Key::UiImages),
        videos: t(Key::UiVideos),
        app_filter: t(Key::UiAppFilter),
        redetect: t(Key::UiRedetect),
        install: t(Key::UiInstall),
        remove: t(Key::UiRemove),
        check_now: t(Key::SetCheckNow),
        github: t(Key::UiGithub),
        logs: t(Key::UiLogs),
        empty: t(Key::UiGalleryEmpty),
        name_hint: t(Key::UiNameHint),
        shortcut_hint: t(Key::UiShortcutHint),
        choose: t(Key::ConvChoose),
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

fn row_model(row: &Row, config: &Config) -> SettingRow {
    let value = row.value(config);
    let mut out = SettingRow {
        id: row.id.into(),
        label: row.label.as_str().into(),
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
        (Kind::Choice(options), Value::Text(t)) => {
            out.kind = 1;
            out.selected = options
                .iter()
                .position(|o| o.value == t)
                .map_or(-1, |i| i as i32);
            out.options = strings(options.iter().map(|o| o.label.clone()));
        }
        (Kind::Number { min, max, step }, Value::Int(n)) => {
            out.kind = 2;
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
            out.text = t.into();
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
        dark: bool,
    ) -> Result<Self, slint::PlatformError> {
        let window = SettingsWindow::new()?;
        let lang = Lang::resolve(&config.general.language, os_locale.as_deref());
        let state = Rc::new(State {
            env: RefCell::new(Env::new(lang, version)),
            config: RefCell::new(config),
            os_locale,
            version: version.to_owned(),
            rows: RefCell::default(),
            selected_profile: RefCell::new(0),
            on_change: RefCell::new(Box::new(|_| {})),
            on_browse: RefCell::new(Box::new(|_| None)),
            on_page_action: RefCell::new(Box::new(|_, _, _| {})),
            on_gallery: RefCell::new(Box::new(|_| {})),
            on_section: RefCell::new(Box::new(|_| {})),
        });
        window.set_dark(dark);
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

    pub fn set_lines(&self, lines: Vec<Line>, selected: Option<usize>) {
        self.window.set_lines(line_model(lines, selected));
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
        *self.state.env.borrow_mut() = Env::new(lang, &self.state.version);
        self.window.set_t(ui_texts(lang));
        self.window
            .set_window_title(tr(Key::SettingsTitle, lang).into());
        self.window.set_section_titles(strings(
            Section::ALL.iter().map(|s| tr(s.title(), lang).to_owned()),
        ));
    }

    /// Rebuilds what the current page shows from the settings.
    pub fn refresh(&self) {
        let state = &self.state;
        let section = self.current_section();
        let config = state.config.borrow();
        let env = state.env.borrow();
        if section.is_rows() {
            let built = rows(section, &env, &config);
            let model: Vec<SettingRow> = built.iter().map(|r| row_model(r, &config)).collect();
            self.window
                .set_rows(ModelRc::from(Rc::new(VecModel::from(model))));
            *state.rows.borrow_mut() = built;
        } else {
            self.window.set_rows(ModelRc::default());
            state.rows.borrow_mut().clear();
        }
        let notice = if section == Section::Video {
            video_problems(&config, (1920, 1080))
                .into_iter()
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        let heading = if notice.is_empty() {
            String::new()
        } else {
            format!("{}\n{notice}", tr(Key::SetProblems, env.lang))
        };
        self.window.set_notice(heading.into());
        match section {
            Section::Shortcuts => {
                drop(config);
                drop(env);
                self.refresh_shortcuts(&[]);
            }
            Section::Profiles => {
                drop(config);
                drop(env);
                self.refresh_profiles();
            }
            Section::Updates => {
                self.window
                    .set_extra(tr(Key::SetUpdatesLater, env.lang).into());
            }
            _ => {}
        }
    }

    fn refresh_shortcuts(&self, errors: &[(usize, String)]) {
        let config = self.state.config.borrow();
        let lang = self.state.lang();
        let table: Vec<ShortcutRow> = shortcuts::table(&config)
            .into_iter()
            .enumerate()
            .map(|(i, (action, slots))| ShortcutRow {
                label: action_label(action, lang).into(),
                s0: slots[0].as_str().into(),
                s1: slots[1].as_str().into(),
                s2: slots[2].as_str().into(),
                error: errors
                    .iter()
                    .find(|(row, _)| *row == i)
                    .map_or_else(SharedString::new, |(_, e)| e.as_str().into()),
            })
            .collect();
        self.window
            .set_shortcuts(ModelRc::from(Rc::new(VecModel::from(table))));
    }

    fn refresh_profiles(&self) {
        let config = self.state.config.borrow();
        let lang = self.state.lang();
        let selected =
            (*self.state.selected_profile.borrow()).min(config.profiles.len().saturating_sub(1));
        let lines: Vec<Line> = config
            .profiles
            .keys()
            .map(|name| {
                let mut tags = Vec::new();
                if *name == config.video.profile {
                    tags.push(tr(Key::ProfileCurrent, lang));
                }
                if *name == config.replay.profile {
                    tags.push(tr(Key::ProfileReplay, lang));
                }
                Line {
                    text: name.clone(),
                    detail: tags.join(", "),
                    strong: *name == config.video.profile,
                }
            })
            .collect();
        drop(config);
        self.set_lines(lines, Some(selected));
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

        w.on_gallery_select({
            let state = self.state.clone();
            move |i| (state.on_gallery.borrow())(GalleryRequest::Select(i.max(0) as usize))
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

        w.on_shortcut_edited({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |row, slot, text| {
                let Some(p) = panel(&weak, &state) else {
                    return;
                };
                let lang = state.lang();
                let Some(action) = ActionId::ALL.get(row as usize).copied() else {
                    return;
                };
                let result =
                    shortcuts::set(&mut state.config.borrow_mut(), action, slot as usize, &text);
                let errors = match result {
                    Ok(()) => {
                        state.changed();
                        Vec::new()
                    }
                    Err(refusal) => vec![(
                        row as usize,
                        match refusal {
                            Refusal::Invalid(message) => message,
                            Refusal::Duplicate => tr(Key::ShortcutDuplicate, lang).to_owned(),
                            Refusal::Conflict(other) => tr(Key::ShortcutConflict, lang)
                                .replace("{action}", action_label(other, lang)),
                        },
                    )],
                };
                p.refresh_shortcuts(&errors);
            }
        });

        w.on_line_select({
            let (weak, state) = (weak.clone(), self.state.clone());
            move |i| {
                if let Some(p) = panel(&weak, &state) {
                    *state.selected_profile.borrow_mut() = i.max(0) as usize;
                    p.refresh_profiles();
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
                if section == Section::Profiles {
                    p.profile_action(&action, &arg);
                } else {
                    (state.on_page_action.borrow())(section, &action, &arg);
                }
            }
        });
    }

    fn profile_action(&self, action: &str, arg: &str) {
        let state = &self.state;
        let lang = state.lang();
        let names: Vec<String> = state.config.borrow().profiles.keys().cloned().collect();
        let selected = names
            .get((*state.selected_profile.borrow()).min(names.len().saturating_sub(1)))
            .cloned()
            .unwrap_or_default();
        let result = {
            let mut config = state.config.borrow_mut();
            match action {
                "new" => profiles::create(&mut config, arg),
                "duplicate" => profiles::duplicate(&mut config, &selected, arg),
                "rename" => profiles::rename(&mut config, &selected, arg),
                "delete" => profiles::delete(&mut config, &selected),
                "use" => {
                    config.video.profile.clone_from(&selected);
                    Ok(())
                }
                _ => Ok(()),
            }
        };
        match result {
            Ok(()) => {
                self.set_extra("");
                state.changed();
            }
            Err(e) => self.set_extra(tr(
                match e {
                    profiles::ProfileError::EmptyName => Key::ProfileEmpty,
                    profiles::ProfileError::Taken => Key::ProfileTaken,
                    profiles::ProfileError::LastOne => Key::ProfileLast,
                    profiles::ProfileError::NotFound => Key::ProfileEmpty,
                },
                lang,
            )),
        }
        self.refresh_profiles();
    }
}
