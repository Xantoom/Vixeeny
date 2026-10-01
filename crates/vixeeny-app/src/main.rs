// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(windows, windows_subsystem = "windows")]
//! On-demand Vixeeny app. M2: only the IPC client side exists; the UI arrives with M3+.
//! The app connects to the daemon, announces itself and exits after
//! `general.app_idle_exit_seconds` without activity (plan section 1.2).

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use anyhow::Context;
use vixeeny_common::config::Config;
use vixeeny_common::ipc::{self, ActionId, AppToDaemon, DaemonToApp, Endpoint, Hello};

fn main() {
    if let Err(e) = run() {
        eprintln!("vixeeny-app: {e:#}");
        std::process::exit(1);
    }
}

/// `--action <name>` (default: open the settings).
fn parse_action() -> anyhow::Result<ActionId> {
    let mut args = std::env::args().skip(1);
    let mut action = ActionId::OpenSettings;
    while let Some(arg) = args.next() {
        if arg == "--action" {
            let name = args.next().context("--action needs a value")?;
            action = ActionId::from_cli_name(&name)
                .with_context(|| format!("unknown action `{name}`"))?;
        }
    }
    Ok(action)
}

fn run() -> anyhow::Result<()> {
    let first_action = parse_action()?;
    let idle = vixeeny_common::paths::config_file()
        .and_then(|path| Config::load(&path).ok())
        .unwrap_or_default()
        .general
        .app_idle_exit_seconds;
    let idle = Duration::from_secs(u64::from(idle));

    let stream = Endpoint::current_user()
        .connect()
        .context("the Vixeeny daemon is not running")?;
    let (mut recv, mut send) = {
        use ipc::StreamTrait;
        stream.split()
    };
    ipc::write_msg(
        &mut send,
        &Hello::App {
            pid: std::process::id(),
        },
    )?;
    ipc::write_msg(&mut send, &AppToDaemon::Ready)?;

    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("ipc-read".into())
        .spawn(move || {
            while let Ok(Some(msg)) = ipc::read_msg::<_, DaemonToApp>(&mut recv) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        })?;

    eprintln!(
        "vixeeny-app {} started for {first_action:?}",
        env!("CARGO_PKG_VERSION")
    );
    loop {
        match rx.recv_timeout(idle) {
            Ok(DaemonToApp::RunAction { action, .. }) => {
                tracing::info!("action {action:?} (no UI before M3)");
            }
            Ok(DaemonToApp::ConfigChanged) => {}
            // The daemon asked us to stop, or went away.
            Ok(DaemonToApp::Shutdown) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {
                ipc::write_msg(&mut send, &AppToDaemon::Idle)?;
                return Ok(());
            }
        }
    }
}
