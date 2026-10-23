// SPDX-License-Identifier: GPL-3.0-or-later
//! The administrator's rights (see `vixeeny_platform::elevation`), each run with them:
//! `--admin register` by the installer, `--admin grant <pid>` by the settings' button (the
//! settings window `pid` closes meanwhile), `--admin remove` by the uninstaller.

use std::time::Duration;

use anyhow::Context;
use vixeeny_common::ipc::ControlRequest;
use vixeeny_platform::elevation;

pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("register") => register(),
        Some("grant") => {
            // The settings window shows again once Vixeeny has the rights: the one that asked
            // must be gone by then, or it would take the new one's place.
            if let Some(pid) = args.get(1).and_then(|p| p.parse().ok()) {
                let (ended, end) = std::sync::mpsc::channel();
                if vixeeny_platform::on_process_exit(pid, move || {
                    let _ = ended.send(());
                }) {
                    let _ = end.recv_timeout(Duration::from_secs(10));
                }
            }
            if let Err(e) = register() {
                // Vixeeny goes on as it was; the settings show that nothing changed.
                let _ = vixeeny_updater::client::tell_daemon(ControlRequest::OpenSettings);
                return Err(e);
            }
            // Started again from here, with the rights.
            vixeeny_updater::client::restart_daemon(
                Duration::from_secs(30 * 60),
                Some(ControlRequest::OpenSettings),
            )
        }
        Some("remove") => elevation::remove_tasks().map_err(Into::into),
        _ => anyhow::bail!("usage: --admin register | grant <pid> | remove"),
    }
}

/// The task that starts this folder's Vixeeny with the rights, for this account.
fn register() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?.with_file_name("Vixeeny.exe");
    elevation::register_task(&exe).context("cannot give Vixeeny the administrator's rights")?;
    tracing::info!("Vixeeny starts with the administrator's rights");
    Ok(())
}
