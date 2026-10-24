// SPDX-License-Identifier: GPL-3.0-or-later
//! `vixeeny-app --update background`: the check the daemon starts after its own start and each
//! wake from sleep. With automatic
//! updates on, a new version is downloaded and verified without a word, and installed at the next
//! start of Vixeeny (or by the Restart button of the settings); otherwise the user is told once.
//!
//! `vixeeny-app --update pending`: run by the daemon as it starts; undoes an install cut short,
//! or installs a downloaded version (the daemon stops, the new one starts).
//!
//! `vixeeny-app --update install [--settings]`: installs the downloaded version (the Restart
//! button of the settings), then shows the settings again.
//!
//! `vixeeny-app --update off`: run by the installer when its updates box is left unchecked;
//! turns the update check off in the user's settings.
//!
//! `vixeeny-app --update quit`: run by the installer; stops Vixeeny the way an update does (a
//! recording ends first), waiting at most half a minute.

use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, tr};
use vixeeny_common::ipc::ControlRequest;
use vixeeny_updater::client;
use vixeeny_updater::state::{self, State};

pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        // Nobody waits for it: it gives way to everything else (a game).
        Some("background") => {
            vixeeny_platform::run_in_background();
            background()
        }
        // A version restored is not installed again at once.
        Some("pending") => match client::recover()? {
            true => Ok(()),
            false => pending(),
        },
        Some("install") => {
            let reopen = args
                .iter()
                .any(|a| a == "--settings")
                .then_some(ControlRequest::OpenSettings);
            client::install(reopen)
        }
        Some("quit") => client::stop_daemon(Some(std::time::Duration::from_secs(30))),
        Some("off") => {
            let path = vixeeny_common::paths::config_file()
                .ok_or_else(|| anyhow::anyhow!("no settings folder"))?;
            let mut config = Config::load(&path)?;
            config.general.check_updates = false;
            config.general.auto_update = false;
            config.save(&path)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn pending() -> anyhow::Result<()> {
    match client::ready() {
        Some(version) => {
            tracing::info!("installing {version}, downloaded before");
            client::install(None)
        }
        None => Ok(()),
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
    let Some(path) = state::file() else {
        return Ok(());
    };
    let mut state = State::load(&path);
    // In Program Files, only a Vixeeny with the administrator's rights can change the files: one
    // without them (a standard account) tells the user instead.
    let writable = std::env::current_exe().is_ok_and(|exe| {
        use vixeeny_platform::elevation::{self, Rights};
        !exe.parent().is_some_and(elevation::protected_install)
            || elevation::rights() == Rights::Elevated
    });
    if config.general.auto_update && writable {
        // A version whose install failed is downloaded again only by hand.
        if state.failed.as_deref() != Some(release.version.as_str()) {
            client::download(&mut |_, _| {})?;
        }
        return Ok(());
    }
    // Told once per version.
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
