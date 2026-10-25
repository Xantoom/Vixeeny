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

pub fn run_child() -> anyhow::Result<()> {
    let config = vixeeny_common::paths::config_file().and_then(|p| Config::load(&p).ok()).unwrap_or_default();
    let lang = crate::lang(&config.general.language);
    let window = vixeeny_ui::WhatsNewWindow::new().map_err(|e| anyhow::anyhow!("window: {e}"))?;
    vixeeny_ui::theme::apply(&window, vixeeny_ui::theme::default_look());
    window.set_heading(tr(Key::WhatsNewTitle, lang).replace("{version}", CURRENT).into());
    window.set_show_label(tr(Key::SetWhatsNew, lang).into());
    window.set_ok_label(tr(Key::UiOk, lang).into());
    window.set_show_again(config.general.whats_new);
    let known = state::file().map(|p| State::load(&p)).and_then(|s| s.notes_of(CURRENT).map(str::to_owned));
    match known {
        Some(notes) => window.set_notes(vixeeny_updater::release::plain_notes(&notes).into()),
        // Not known yet (the version before this one kept none): asked for while the window shows.
        None => {
            window.set_notes(tr(Key::WhatsNewLoading, lang).into());
            let weak = window.as_weak();
            std::thread::spawn(move || {
                let text = match vixeeny_updater::client::notes(CURRENT) {
                    Ok(notes) if !notes.is_empty() => vixeeny_updater::release::plain_notes(&notes),
                    Ok(_) => String::new(),
                    Err(e) => {
                        tracing::warn!("what's new: {e:#}");
                        tr(Key::WhatsNewFailed, lang).to_owned()
                    }
                };
                let _ = weak.upgrade_in_event_loop(move |w| w.set_notes(text.into()));
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
                crate::settings::save(&config);
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
