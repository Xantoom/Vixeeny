// SPDX-License-Identifier: GPL-3.0-or-later
//! The update check. The network client lives in the app: the daemon only starts `vixeeny-app --update background` (a
//! windowed program, so nothing shows, at the lowest priority) when something may have changed, as Windows reports it:
//! the daemon's own start, each wake from sleep, and each new network address (a connection that comes back, after a
//! start or a wake without one). No timer: the app asks GitHub only when its last answer is 12 hours old. At its start
//! the daemon has the app install a version downloaded before (`--update pending`).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};

use vixeeny_common::config::Config;

/// Tells the checking thread that a check may be due (a wake from sleep, a new network address).
static NUDGE: OnceLock<Sender<()>> = OnceLock::new();

fn app_next_to_daemon() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.with_file_name("vixeeny-app.exe");
    path.exists().then_some(path)
}

/// Starts the checking thread (nothing happens when the setting is off).
pub fn spawn(config_path: Option<PathBuf>) {
    let Some(app) = app_next_to_daemon() else {
        return;
    };
    let (nudge, nudges) = channel();
    let _ = NUDGE.set(nudge);
    let result = std::thread::Builder::new()
        .name("update-check".into())
        .stack_size(128 * 1024)
        .spawn(move || checks(&app, config_path.as_deref(), &nudges));
    match result {
        Ok(_) => {
            on_wake();
            on_network();
        }
        Err(e) => tracing::warn!("cannot start the update check: {e}"),
    }
}

fn checks(app: &std::path::Path, config_path: Option<&std::path::Path>, nudges: &Receiver<()>) {
    // A version downloaded before goes in now (this daemon stops, the new one starts).
    match std::process::Command::new(app).args(["--update", "pending"]).spawn() {
        Ok(mut child) => {
            let _ = child.wait();
        }
        Err(e) => tracing::warn!("cannot start the pending update: {e}"),
    }
    loop {
        let enabled = config_path.and_then(|p| Config::load(p).ok()).is_none_or(|c| c.general.check_updates);
        if enabled {
            match std::process::Command::new(app).args(["--update", "background"]).spawn() {
                Ok(mut child) => {
                    let _ = child.wait();
                }
                Err(e) => tracing::warn!("cannot start the update check: {e}"),
            }
        }
        // Blocked until Windows reports something; what came during the check counts as done.
        while nudges.try_recv().is_ok() {}
        if nudges.recv().is_err() {
            return;
        }
    }
}

fn nudge() {
    if let Some(nudge) = NUDGE.get() {
        let _ = nudge.send(());
    }
}

/// Has Windows report each wake from sleep to the checking thread.
fn on_wake() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Power::{DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification};
    use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC};
    unsafe extern "system" fn woke(_: *const std::ffi::c_void, kind: u32, _: *const std::ffi::c_void) -> u32 {
        if kind == PBT_APMRESUMEAUTOMATIC {
            nudge();
        }
        0
    }
    // Kept for the life of the daemon, as the registration.
    let params: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
        Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(woke), Context: std::ptr::null_mut() }));
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

/// Has Windows report each new network address to the checking thread: a connection that comes back.
fn on_network() {
    use windows::Win32::Foundation::{HANDLE, NO_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        MIB_NOTIFICATION_TYPE, MIB_UNICASTIPADDRESS_ROW, MibAddInstance, NotifyUnicastIpAddressChange,
    };
    use windows::Win32::Networking::WinSock::AF_UNSPEC;
    unsafe extern "system" fn changed(
        _: *const std::ffi::c_void,
        _: *const MIB_UNICASTIPADDRESS_ROW,
        kind: MIB_NOTIFICATION_TYPE,
    ) {
        if kind == MibAddInstance {
            nudge();
        }
    }
    let mut handle = HANDLE::default();
    // SAFETY: a callback that lives as long as the process; the registration is never undone.
    let status = unsafe { NotifyUnicastIpAddressChange(AF_UNSPEC, Some(changed), None, false, &raw mut handle) };
    if status != NO_ERROR {
        tracing::warn!("no network notifications: the update check runs at start and wake only");
    }
}
