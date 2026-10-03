// SPDX-License-Identifier: GPL-3.0-or-later
//! Desktop notifications on Linux: the Notification portal inside a sandbox (Flatpak has no
//! `notify-send`), `notify-send` (libnotify) elsewhere, which reaches whatever notification
//! daemon the desktop runs.

use std::process::{Command, Stdio};

/// Shows `body` under `title`. Never blocks for long; an `Err` says why nothing was shown.
pub fn notify(title: &str, body: &str) -> Result<(), String> {
    if ashpd::is_sandboxed() {
        return via_portal(title, body);
    }
    let status = Command::new("notify-send")
        .args([
            "--app-name=Vixeeny",
            "--icon=io.github.Xantoom.Vixeeny",
            title,
            body,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("notify-send: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("notify-send exited with {status}"))
    }
}

fn via_portal(title: &str, body: &str) -> Result<(), String> {
    use ashpd::desktop::notification::{Notification, NotificationProxy};

    // The same id replaces the previous banner instead of piling them up.
    async_io::block_on(async {
        let proxy = NotificationProxy::new().await?;
        proxy
            .add_notification("vixeeny", Notification::new(title).body(body))
            .await
    })
    .map_err(|e| format!("notification portal: {e}"))
}
