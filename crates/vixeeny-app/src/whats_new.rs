// SPDX-License-Identifier: GPL-3.0-or-later
//! `vixeeny-app --whats-new`: the window shown at the first start after an update, with what changed in the new
//! version. Its switch is the one of the updates page (`general.whats_new`).

use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, tr};
use vixeeny_ui::ComponentHandle;
use vixeeny_updater::state::{self, State};

const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Called as Vixeeny starts: records the version running, and opens the window when it is new since the last start and
/// the user wants it.
pub fn on_start(config: &Config) {
    let Some(path) = state::file() else {
        return;
    };
    let mut state = State::load(&path);
    let arrived = state.arrived(CURRENT);
    take_print_screen_once(&mut state);
    if let Err(e) = state.save(&path) {
        tracing::warn!("update state: {e}");
        return;
    }
    if arrived && config.general.whats_new {
        tracing::info!("updated to {CURRENT}: what's new");
        if let Err(e) =
            std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--whats-new").spawn())
        {
            tracing::warn!("cannot show what's new: {e}");
        }
    }
}

/// Once per user, whatever installed Vixeeny (the installer does it too, an update or a portable copy does not): the
/// Print Screen key opens Windows' Snipping Tool no more, over Vixeeny's own capture. Tried again at the next start
/// when Windows refused.
fn take_print_screen_once(state: &mut State) {
    if state.print_screen_taken {
        return;
    }
    match take_print_screen(state) {
        Ok(()) => state.print_screen_taken = true,
        Err(e) => tracing::warn!("cannot turn off the Snipping Tool's Print Screen key: {e}"),
    }
}

/// Takes the Print Screen key from Windows' Snipping Tool when it has it, and records that it had it (the uninstaller
/// gives it back, see [`give_print_screen`]).
fn take_print_screen(state: &mut State) -> Result<(), vixeeny_platform::PlatformError> {
    if !vixeeny_platform::print_screen_opens_snipping_tool() {
        return Ok(());
    }
    vixeeny_platform::take_print_screen()?;
    tracing::info!("the Print Screen key no longer opens Windows' Snipping Tool");
    state.print_screen_was_on = true;
    Ok(())
}

/// `vixeeny-app --print-screen take`, run by the installer.
pub fn take_print_screen_now() -> anyhow::Result<()> {
    let path = state::file().ok_or_else(|| anyhow::anyhow!("no settings folder"))?;
    let mut state = State::load(&path);
    take_print_screen(&mut state)?;
    state.print_screen_taken = true;
    state.save(&path)?;
    Ok(())
}

/// `vixeeny-app --print-screen give`, run by the uninstaller: the Print Screen key opens the Snipping Tool again when
/// it did before Vixeeny took it.
pub fn give_print_screen() -> anyhow::Result<()> {
    let Some(path) = state::file() else {
        return Ok(());
    };
    let mut state = State::load(&path);
    if !state.print_screen_was_on {
        return Ok(());
    }
    vixeeny_platform::give_print_screen()?;
    state.print_screen_was_on = false;
    state.print_screen_taken = false;
    state.save(&path)?;
    Ok(())
}

/// Markdown release notes as the windows show them.
pub fn ui_notes(notes: &str) -> Vec<vixeeny_ui::NoteLine> {
    use vixeeny_updater::release::{NoteKind, note_lines};
    note_lines(notes)
        .into_iter()
        .map(|line| {
            let (kind, level) = match line.kind {
                NoteKind::Text => (0, 0),
                NoteKind::Heading => (1, 0),
                NoteKind::Item { level } => (2, i32::from(level)),
            };
            vixeeny_ui::NoteLine { kind, level, text: line.text.into() }
        })
        .collect()
}

fn lines_model(lines: Vec<vixeeny_ui::NoteLine>) -> vixeeny_ui::slint::ModelRc<vixeeny_ui::NoteLine> {
    vixeeny_ui::slint::ModelRc::from(std::rc::Rc::new(vixeeny_ui::slint::VecModel::from(lines)))
}

pub fn run_child(config: &Config) -> anyhow::Result<()> {
    let lang = crate::lang(&config.general.language);
    let window = vixeeny_ui::WhatsNewWindow::new().map_err(|e| anyhow::anyhow!("window: {e}"))?;
    vixeeny_ui::theme::apply(&window, vixeeny_ui::theme::default_look());
    vixeeny_ui::theme::watch(&window);
    window.set_heading(tr(Key::WhatsNewTitle, lang).replace("{version}", CURRENT).into());
    window.set_show_label(tr(Key::SetWhatsNew, lang).into());
    window.set_ok_label(tr(Key::UiOk, lang).into());
    window.set_show_again(config.general.whats_new);
    // Built into the program; else kept when it was downloaded; else asked for.
    let known = vixeeny_updater::changelog::running(lang)
        .map(str::to_owned)
        .or_else(|| state::file().map(|p| State::load(&p)).and_then(|s| s.notes_of(CURRENT, lang).map(str::to_owned)));
    match known {
        Some(notes) => window.set_notes(lines_model(ui_notes(&notes))),
        // Not known yet (the version before this one kept none): asked for while the window shows.
        None => {
            window.set_message(tr(Key::WhatsNewLoading, lang).into());
            let weak = window.as_weak();
            std::thread::spawn(move || {
                let (lines, message) = match vixeeny_updater::client::notes(CURRENT, lang) {
                    Ok(notes) => (ui_notes(&notes), String::new()),
                    Err(e) => {
                        tracing::warn!("what's new: {e:#}");
                        (Vec::new(), tr(Key::WhatsNewFailed, lang).to_owned())
                    }
                };
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_notes(lines_model(lines));
                    w.set_message(message.into());
                });
            });
        }
    }
    window.on_show_toggled(|on| {
        let Some(path) = vixeeny_common::paths::config_file() else {
            return;
        };
        match Config::load(&path) {
            Ok(mut config) => {
                config.general.whats_new = on;
                let _ = crate::settings::save(&config);
            }
            Err(e) => tracing::warn!("cannot read the settings: {e}"),
        }
    });
    let weak = window.as_weak();
    window.on_done(move || {
        if let Some(w) = weak.upgrade() {
            let _ = w.hide();
        }
    });
    window.run().map_err(|e| anyhow::anyhow!("window: {e}"))?;
    crate::settings::finish_saving();
    Ok(())
}
