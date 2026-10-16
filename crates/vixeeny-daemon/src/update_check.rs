// SPDX-License-Identifier: GPL-3.0-or-later
//! The update check. The network client lives in the app: the daemon only starts
//! `vixeeny-app --update background` (a windowed program, so nothing shows) a moment after its
//! own start and after each wake from sleep, which Windows reports (nothing is timed in
//! between); the app checks and downloads, or tells the user. At its start the daemon has the
//! app install a version downloaded before (`--update pending`).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use vixeeny_common::config::Config;

/// The check waits this long after the start or the wake, so it never competes with it (and
/// the network is back).
const SETTLE: Duration = Duration::from_secs(90);

/// Tells the checking thread that the computer woke up.
static WOKE: OnceLock<Mutex<Sender<()>>> = OnceLock::new();

fn app_next_to_daemon() -> Option<PathBuf> {
    let path = std::env::current_exe()
        .ok()?
        .with_file_name("vixeeny-app.exe");
    path.exists().then_some(path)
}

/// Starts the checking thread (nothing happens when the setting is off).
pub fn spawn(config_path: Option<PathBuf>) {
    let Some(app) = app_next_to_daemon() else {
        return;
    };
    let (woke, wakes) = channel();
    let _ = WOKE.set(Mutex::new(woke));
    let result = std::thread::Builder::new()
        .name("update-check".into())
        .stack_size(128 * 1024)
        .spawn(move || checks(&app, config_path.as_deref(), &wakes));
    match result {
        Ok(_) => on_wake(),
        Err(e) => tracing::warn!("cannot start the update check: {e}"),
    }
}

fn checks(app: &std::path::Path, config_path: Option<&std::path::Path>, wakes: &Receiver<()>) {
    // A version downloaded before goes in now (this daemon stops, the new one starts).
    match std::process::Command::new(app)
        .args(["--update", "pending"])
        .spawn()
    {
        Ok(mut child) => {
            let _ = child.wait();
        }
        Err(e) => tracing::warn!("cannot start the pending update: {e}"),
    }
    loop {
        std::thread::sleep(SETTLE);
        // Wakes that came meanwhile count as this one.
        while wakes.try_recv().is_ok() {}
        let enabled = config_path
            .and_then(|p| Config::load(p).ok())
            .is_none_or(|c| c.general.check_updates);
        if enabled {
            match std::process::Command::new(app)
                .args(["--update", "background"])
                .spawn()
            {
                Ok(mut child) => {
                    let _ = child.wait();
                }
                Err(e) => tracing::warn!("cannot start the update check: {e}"),
            }
        }
        if wakes.recv().is_err() {
            return;
        }
    }
}

/// Has Windows report each wake from sleep to the checking thread.
#[cfg(windows)]
fn on_wake() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC};
    unsafe extern "system" fn woke(
        _: *const std::ffi::c_void,
        kind: u32,
        _: *const std::ffi::c_void,
    ) -> u32 {
        if kind == PBT_APMRESUMEAUTOMATIC
            && let Some(Ok(woke)) = WOKE.get().map(Mutex::lock)
        {
            let _ = woke.send(());
        }
        0
    }
    // Kept for the life of the daemon, as the registration.
    let params: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
        Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(woke),
            Context: std::ptr::null_mut(),
        }));
    let mut registration = std::ptr::null_mut();
    // SAFETY: `params` lives as long as the process; the registration is never undone.
    let status = unsafe {
        PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE((&raw mut *params).cast()),
            &raw mut registration,
        )
    };
    if status.is_err() {
        tracing::warn!("no wake-from-sleep notifications: the update check runs at start only");
    }
}

#[cfg(not(windows))]
fn on_wake() {}
