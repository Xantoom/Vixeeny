// SPDX-License-Identifier: GPL-3.0-or-later
//! Wayland shortcuts through the GlobalShortcuts portal: the compositor owns the keys, asks the
//! user to confirm them, and tells us when one is pressed. Compositors without the portal
//! (older GNOME, some wlroots ones) can bind `vixeeny-daemon ctl <action>` to a key instead.

use std::collections::HashMap;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_lite::{StreamExt, future};
use vixeeny_common::hotkey::Hotkey;
use vixeeny_common::ipc::ActionId;

use crate::core::Event;
use crate::server::EventTx;

/// How often the session thread looks at its stop flag while no shortcut is pressed.
const STOP_POLL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct PortalShortcuts {
    stop: Option<Arc<AtomicBool>>,
}

/// The portal's trigger syntax (`CTRL+ALT+s`) for a shortcut. Only a preference: the user
/// confirms or changes it in the compositor's dialog.
fn trigger(hotkey: &Hotkey) -> String {
    let mut parts = Vec::new();
    let m = hotkey.mods;
    for (on, name) in [
        (m.ctrl, "CTRL"),
        (m.alt, "ALT"),
        (m.shift, "SHIFT"),
        (m.meta, "LOGO"),
    ] {
        if on {
            parts.push(name.to_string());
        }
    }
    let key = hotkey
        .code
        .strip_prefix("Key")
        .or_else(|| hotkey.code.strip_prefix("Digit"))
        .unwrap_or(&hotkey.code);
    parts.push(if key.len() == 1 {
        key.to_lowercase()
    } else {
        key.to_string()
    });
    parts.join("+")
}

async fn session(
    bindings: Vec<(ActionId, String)>,
    tx: EventTx,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(Default::default()).await?;
    let mut activated = pin!(portal.receive_activated().await?);
    let shortcuts: Vec<NewShortcut> = bindings
        .iter()
        .map(|(action, trigger)| {
            NewShortcut::new(action.cli_name(), action.cli_name())
                .preferred_trigger(trigger.as_str())
        })
        .collect();
    portal
        .bind_shortcuts(&session, &shortcuts, None, Default::default())
        .await?;
    let actions: HashMap<&str, ActionId> =
        bindings.iter().map(|(a, _)| (a.cli_name(), *a)).collect();
    while !stop.load(Ordering::Relaxed) {
        let next = future::or(async { Some(activated.next().await) }, async {
            async_io::Timer::after(STOP_POLL).await;
            None
        })
        .await;
        match next {
            Some(Some(event)) => {
                if let Some(action) = actions.get(event.shortcut_id()) {
                    tx.send(Event::Action(*action));
                }
            }
            Some(None) => break,
            None => {}
        }
    }
    session.close().await?;
    Ok(())
}

impl PortalShortcuts {
    /// Replaces the portal session by one with `bindings`. The portal may show a dialog; the
    /// outcome is only logged because it comes much later than this call.
    pub fn apply(&mut self, bindings: &[(ActionId, Hotkey)], tx: &EventTx) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if bindings.is_empty() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        let bindings: Vec<_> = bindings.iter().map(|(a, h)| (*a, trigger(h))).collect();
        let tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("portal-shortcuts".into())
            .stack_size(512 * 1024)
            .spawn(move || {
                if let Err(e) = async_io::block_on(session(bindings, tx, stop)) {
                    tracing::warn!(
                        "the GlobalShortcuts portal is not usable ({e}); bind `vixeeny-daemon ctl <action>` to a key in your compositor instead"
                    );
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("cannot start the shortcuts thread: {e}");
        }
    }
}

impl Drop for PortalShortcuts {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
    }
}
