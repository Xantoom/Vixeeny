// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(windows, windows_subsystem = "windows")]
//! The always-on Vixeeny daemon (plan section 1.2).

use vixeeny_common::config::Config;
use vixeeny_common::ipc::{ActionId, BindError, ControlRequest, Endpoint};
use vixeeny_daemon::platform::{self, Startup};
use vixeeny_daemon::server::{AppLink, ask_running_daemon};

fn main() {
    vixeeny_common::logging::init("vixeeny-daemon");
    if let Err(e) = real_main() {
        tracing::error!("fatal: {e:#}");
        std::process::exit(1);
    }
}

/// `vixeeny-daemon ctl <action>`: asks the running daemon to run an action, for a compositor
/// shortcut (Wayland) or a script. `ctl` alone lists the actions.
fn ctl(args: &[String]) -> anyhow::Result<()> {
    let Some(action) = args.first().and_then(|name| ActionId::from_cli_name(name)) else {
        let names: Vec<_> = ActionId::ALL.iter().map(|a| a.cli_name()).collect();
        anyhow::bail!("usage: vixeeny-daemon ctl <{}>", names.join("|"));
    };
    ask_running_daemon(&Endpoint::current_user(), ControlRequest::Action(action))
        .map_err(|e| anyhow::anyhow!("the Vixeeny daemon does not answer ({e}); is it running?"))
}

fn real_main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "ctl") {
        return ctl(&args[1..]);
    }
    let endpoint = Endpoint::current_user();
    // Binding the endpoint is the single-instance lock.
    let listener = match endpoint.bind() {
        Ok(listener) => listener,
        Err(BindError::AlreadyRunning) => {
            tracing::info!("daemon already running, asking it to open the settings");
            ask_running_daemon(&endpoint, ControlRequest::OpenSettings)?;
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    let config_path = vixeeny_common::paths::config_file();
    let config = match &config_path {
        Some(path) => Config::load(path).unwrap_or_else(|e| {
            tracing::warn!("using the default configuration: {e}");
            Config::default()
        }),
        None => Config::default(),
    };
    tracing::info!("daemon started (pid {})", std::process::id());
    platform::run(Startup {
        config,
        config_path,
        listener,
        link: AppLink::default(),
    })
}
