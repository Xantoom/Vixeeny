// SPDX-License-Identifier: GPL-3.0-or-later
//! `vixeeny-app --update background`: the check the daemon starts every 12 hours. With automatic
//! updates on, a new version is downloaded and verified without a word, and installed at the next
//! start of Vixeeny (or by the Restart button of the settings); otherwise the user is told once.
//!
//! `vixeeny-app --update pending`: run by the daemon as it starts; installs a downloaded version
//! (the daemon stops, the new one starts).
//!
//! `vixeeny-app --update install [--settings]`: installs the downloaded version (the Restart
//! button of the settings), then shows the settings again.

use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, tr};
use vixeeny_common::ipc::ControlRequest;
use vixeeny_updater::client;
use vixeeny_updater::state::{self, State};

pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("background") => background(),
        Some("pending") => match client::ready() {
            Some(version) => {
                tracing::info!("installing {version}, downloaded before");
                client::install(None)
            }
            None => Ok(()),
        },
        Some("install") => {
            let reopen = args
                .iter()
                .any(|a| a == "--settings")
                .then_some(ControlRequest::OpenSettings);
            client::install(reopen)
        }
        _ => Ok(()),
    }
}

fn background() -> anyhow::Result<()> {
    client::clean_up();
    let config = vixeeny_common::paths::config_file()
        .and_then(|p| Config::load(&p).ok())
        .unwrap_or_default();
    if !config.general.check_updates {
        return Ok(());
    }
    let Some(release) = client::check()? else {
        return Ok(());
    };
    tracing::info!("version {} is available", release.version);
    if config.general.auto_update {
        client::download(&mut |_, _| {})?;
        return Ok(());
    }
    // Told once per version.
    let Some(path) = state::file() else {
        return Ok(());
    };
    let mut state = State::load(&path);
    if state.notified.as_deref() == Some(release.version.as_str()) {
        return Ok(());
    }
    state.notified = Some(release.version.clone());
    state.save(&path)?;
    let lang = crate::lang(&config.general.language);
    let text = tr(Key::UpdateAvailable, lang).replace("{version}", &release.version);
    crate::toast::notify(&config, &crate::toast::Toast::Update(text));
    Ok(())
}

/// Starts the installation in a process of its own (the settings window closes meanwhile and
/// comes back with the new version).
pub fn spawn_install() -> anyhow::Result<()> {
    std::process::Command::new(std::env::current_exe()?)
        .args(["--update", "install", "--settings"])
        .spawn()?;
    Ok(())
}
