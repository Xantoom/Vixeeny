// SPDX-License-Identifier: GPL-3.0-or-later
//! The first-run assistant (plan 5.15): language, folders, start-up, hardware.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};

use crate::WizardWindow;

/// Language setting behind each entry of the list.
const LANGUAGES: [&str; 3] = ["auto", "fr", "en"];
/// Steps: language, folders, start-up, hardware.
pub const STEPS: usize = 4;

/// Picks a folder; `None` when the user cancels. The argument is the current one.
pub type BrowseFn = dyn Fn(&str) -> Option<String>;

struct State {
    /// What the user chose so far.
    config: Config,
    /// How the settings were when the assistant opened.
    original: Config,
    os_locale: Option<String>,
    step: usize,
    hardware: String,
    finished: bool,
}

impl State {
    fn lang(&self) -> Lang {
        Lang::resolve(&self.config.general.language, self.os_locale.as_deref())
    }
}

/// A thread-safe way to give the hardware summary to the window.
#[derive(Clone)]
pub struct WizardHandle(slint::Weak<WizardWindow>);

impl WizardHandle {
    pub fn set_hardware(&self, text: String) {
        let _ = self.0.upgrade_in_event_loop(move |w| {
            w.set_hardware_text(text.into());
        });
    }
}

pub struct WizardPanel {
    window: WizardWindow,
    state: Rc<RefCell<State>>,
}

fn show(window: &WizardWindow, state: &State) {
    let lang = state.lang();
    let step = state.step;
    window.set_step(step as i32);
    window.set_last(step + 1 == STEPS);
    let (heading, body) = match step {
        0 => (Key::WizWelcomeTitle, Some(Key::WizWelcomeBody)),
        1 => (Key::WizFoldersTitle, None),
        2 => (Key::WizStartupTitle, Some(Key::WizStartupBody)),
        _ => (Key::WizHardwareTitle, None),
    };
    window.set_heading(tr(heading, lang).into());
    window.set_body(body.map_or("", |k| tr(k, lang)).into());
    window.set_step_text(
        tr(Key::WizStepOf, lang)
            .replace("{n}", &(step + 1).to_string())
            .replace("{total}", &STEPS.to_string())
            .into(),
    );
    window.set_language_label(tr(Key::WizLanguageLabel, lang).into());
    let names: Vec<SharedString> = vec![
        tr(Key::WizLanguageAuto, lang).into(),
        "Français".into(),
        "English".into(),
    ];
    window.set_languages(ModelRc::new(VecModel::from(names)));
    let chosen = LANGUAGES
        .iter()
        .position(|l| *l == state.config.general.language)
        .unwrap_or(0);
    window.set_language(chosen as i32);
    window.set_images_label(tr(Key::WizImagesLabel, lang).into());
    window.set_videos_label(tr(Key::WizVideosLabel, lang).into());
    window.set_images_path(state.config.paths.images.as_str().into());
    window.set_videos_path(state.config.paths.videos.as_str().into());
    window.set_browse_label(tr(Key::WizBrowse, lang).into());
    window.set_autostart_label(tr(Key::WizAutostart, lang).into());
    window.set_autostart(state.config.general.autostart);
    window.set_notifications_label(tr(Key::WizNotifications, lang).into());
    window.set_notifications(state.config.general.notifications);
    window.set_back_label(tr(Key::WizBack, lang).into());
    window.set_next_label(tr(Key::WizNext, lang).into());
    window.set_finish_label(tr(Key::WizFinish, lang).into());
    window.set_skip_label(tr(Key::WizSkip, lang).into());
    let hardware = if state.hardware.is_empty() {
        tr(Key::HwDetecting, lang)
    } else {
        state.hardware.as_str()
    };
    window.set_hardware_text(hardware.into());
}

impl WizardPanel {
    pub fn new(
        config: Config,
        os_locale: Option<String>,
        look: crate::theme::Look,
        browse: Box<BrowseFn>,
    ) -> Result<Self, slint::PlatformError> {
        let window = WizardWindow::new()?;
        crate::theme::apply(&window, look);
        let state = Rc::new(RefCell::new(State {
            original: config.clone(),
            config,
            os_locale,
            step: 0,
            hardware: String::new(),
            finished: false,
        }));
        show(&window, &state.borrow());

        let (w, s) = (window.as_weak(), state.clone());
        let refresh = move |change: &dyn Fn(&mut State)| {
            let mut state = s.borrow_mut();
            change(&mut state);
            if let Some(w) = w.upgrade() {
                show(&w, &state);
            }
        };
        let refresh = Rc::new(refresh);

        let r = refresh.clone();
        window.on_language_chosen(move |i| {
            r(&|s| {
                if let Some(tag) = LANGUAGES.get(i as usize) {
                    s.config.general.language = (*tag).to_owned();
                }
            });
        });
        let r = refresh.clone();
        window.on_browse(move |which| {
            r(&|s| {
                let current = if which == 0 {
                    &mut s.config.paths.images
                } else {
                    &mut s.config.paths.videos
                };
                if let Some(picked) = browse(current) {
                    *current = picked;
                }
            });
        });
        let r = refresh.clone();
        window.on_autostart_toggled(move |on| r(&|s| s.config.general.autostart = on));
        let r = refresh.clone();
        window.on_notifications_toggled(move |on| r(&|s| s.config.general.notifications = on));
        let r = refresh.clone();
        window.on_back(move || r(&|s| s.step = s.step.saturating_sub(1)));
        let (r, s, w) = (refresh.clone(), state.clone(), window.as_weak());
        window.on_next(move || {
            r(&|s| {
                if s.step + 1 < STEPS {
                    s.step += 1;
                } else {
                    s.finished = true;
                }
            });
            if s.borrow().finished
                && let Some(w) = w.upgrade()
            {
                let _ = w.hide();
            }
        });
        let (r, w) = (refresh.clone(), window.as_weak());
        window.on_skip(move || {
            r(&|s| {
                s.config = s.original.clone();
                s.finished = true;
            });
            if let Some(w) = w.upgrade() {
                let _ = w.hide();
            }
        });
        Ok(Self { window, state })
    }

    pub fn window(&self) -> &WizardWindow {
        &self.window
    }

    pub fn handle(&self) -> WizardHandle {
        WizardHandle(self.window.as_weak())
    }

    /// What the user has chosen so far.
    pub fn choices(&self) -> Config {
        self.state.borrow().config.clone()
    }

    /// "Finish" or "Skip" was pressed.
    pub fn is_finished(&self) -> bool {
        self.state.borrow().finished
    }

    /// Shows the assistant until it is finished, skipped or closed; gives the settings to save
    /// (the choices after "Finish", the original ones otherwise) with the assistant marked done.
    pub fn run(&self) -> Result<Config, slint::PlatformError> {
        self.window.run()?;
        let state = self.state.borrow();
        let mut config = if state.finished {
            state.config.clone()
        } else {
            state.original.clone()
        };
        config.general.first_run_done = true;
        Ok(config)
    }
}
