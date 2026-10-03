// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS: the main thread runs the `NSApplication` loop (no Dock icon: an accessory app with a
//! menu-bar item). Other threads wake the runtime by queueing a block on the main dispatch
//! queue, so nothing polls.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::channel;

use anyhow::Context;
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

use super::Startup;
use super::desktop::{DesktopHotkeys, DesktopTray};
use crate::runtime::{Flow, Runtime};
use crate::server::{self, EventTx, Waker};
use crate::supervisor::ProcessSpawner;

type DaemonRuntime = Runtime<DesktopTray, ProcessSpawner>;

thread_local! {
    static RUNTIME: RefCell<Option<DaemonRuntime>> = const { RefCell::new(None) };
}

/// Runs the runtime's pump on the main thread.
struct MainQueueWaker;

impl Waker for MainQueueWaker {
    fn wake(&self) {
        DispatchQueue::main().exec_async(|| {
            // `try_borrow_mut`: a re-entrant wake is picked up by the pump already running.
            let flow = RUNTIME.with(|cell| {
                cell.try_borrow_mut()
                    .ok()
                    .and_then(|mut rt| rt.as_mut().map(Runtime::pump))
            });
            if flow == Some(Flow::Quit) {
                // Drop the runtime (and the menu-bar item) so the icon disappears at once.
                RUNTIME.with(|cell| cell.borrow_mut().take());
                std::process::exit(0);
            }
        });
    }
}

pub fn run(startup: Startup) -> anyhow::Result<()> {
    let Startup {
        config,
        config_path,
        listener,
        link,
    } = startup;
    let mtm = MainThreadMarker::new().context("the daemon must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let (sender, receiver) = channel();
    let tx = EventTx::new(sender, Arc::new(MainQueueWaker));

    let os_locale = vixeeny_platform::user_locale();
    let lang = vixeeny_common::i18n::Lang::resolve(&config.general.language, os_locale.as_deref());
    let tray = DesktopTray::new(lang, &tx)?;
    let hotkeys = DesktopHotkeys::new(&tx)?;
    let spawner = ProcessSpawner::next_to_current_exe().context("locating vixeeny-app")?;

    let serve_tx = tx.clone();
    let serve_link = link.clone();
    std::thread::Builder::new()
        .name("ipc-accept".into())
        .stack_size(256 * 1024)
        .spawn(move || server::serve(&listener, &serve_tx, &serve_link))
        .context("starting the IPC thread")?;

    crate::update_check::spawn(config_path.clone(), tx.clone());
    let mut runtime = Runtime::new(
        config,
        config_path,
        os_locale,
        tray,
        Box::new(hotkeys),
        spawner,
        link,
        tx,
        receiver,
    );
    runtime.apply_config();
    runtime.start_replay_if_configured();
    runtime.open_wizard_if_first_run();
    RUNTIME.with(|cell| *cell.borrow_mut() = Some(runtime));

    app.run();
    RUNTIME.with(|cell| cell.borrow_mut().take());
    Ok(())
}
