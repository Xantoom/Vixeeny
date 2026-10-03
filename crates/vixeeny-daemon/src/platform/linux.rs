// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux: no GUI toolkit in the daemon. The tray is a StatusNotifierItem served over D-Bus by
//! `ksni` (KDE, most GNOME setups with the AppIndicator extension, waybar, ...), the shortcuts
//! are X11 key grabs or the GlobalShortcuts portal, and the loop is a plain channel: any thread
//! wakes it by sending a token, and it sleeps in `recv` otherwise.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use anyhow::Context;
use ksni::blocking::{Handle, TrayMethods};
use vixeeny_common::hotkey::Hotkey;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::{ActionId, RecState};

use super::Startup;
use super::hotkeys::DesktopHotkeys;
use super::portal_shortcuts::PortalShortcuts;
use crate::core::Event;
use crate::icon;
use crate::runtime::{Flow, HotkeyBackend, Runtime, Tray};
use crate::server::{self, EventTx, Waker};
use crate::supervisor::ProcessSpawner;

struct ChannelWaker(Sender<()>);

impl Waker for ChannelWaker {
    fn wake(&self) {
        // The receiver is gone only while the daemon shuts down.
        let _ = self.0.send(());
    }
}

/// `LC_ALL`, `LC_MESSAGES` then `LANG`, as the C library would pick them.
fn os_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
        .map(|value| value.split('.').next().unwrap_or(&value).replace('_', "-"))
}

/// The state ksni reads each time it publishes the item.
struct TrayState {
    tx: EventTx,
    lang: Lang,
    state: RecState,
    message: Option<String>,
}

impl ksni::Tray for TrayState {
    fn id(&self) -> String {
        "vixeeny".into()
    }

    fn title(&self) -> String {
        "Vixeeny".into()
    }

    fn icon_name(&self) -> String {
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // The item wants ARGB32 in network byte order; the icon is drawn as RGBA.
        let mut data = icon::render(self.state == RecState::Recording);
        for px in data.as_chunks_mut::<4>().0 {
            px.rotate_right(1);
        }
        vec![ksni::Icon {
            width: icon::SIZE as i32,
            height: icon::SIZE as i32,
            data,
        }]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let key = if self.state == RecState::Idle {
            Key::TrayTooltip
        } else {
            Key::TrayTooltipRecording
        };
        ksni::ToolTip {
            title: tr(key, self.lang).to_string(),
            description: self.message.clone().unwrap_or_default(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.tx.send(Event::Action(ActionId::OpenSettings));
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: tr(Key::MenuSettings, self.lang).to_string(),
                activate: Box::new(|t: &mut Self| t.tx.send(Event::Action(ActionId::OpenSettings))),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: tr(Key::MenuQuit, self.lang).to_string(),
                activate: Box::new(|t: &mut Self| t.tx.send(Event::TrayQuit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

struct LinuxTray {
    handle: Option<Handle<TrayState>>,
}

impl LinuxTray {
    fn new(lang: Lang, tx: &EventTx) -> Self {
        let state = TrayState {
            tx: tx.clone(),
            lang,
            state: RecState::Idle,
            message: None,
        };
        // Without a StatusNotifierWatcher (a bare window manager) there is no tray; the daemon
        // still serves shortcuts and `ctl`, so this is not fatal.
        // A sandbox may not own the well-known name the specification asks for.
        let spawned = state.disable_dbus_name(ashpd::is_sandboxed()).spawn();
        let handle = match spawned {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!("no system tray available: {e}");
                None
            }
        };
        Self { handle }
    }

    fn update(&self, f: impl FnOnce(&mut TrayState)) {
        if let Some(handle) = &self.handle {
            handle.update(f);
        }
    }
}

impl Tray for LinuxTray {
    fn set_recording(&mut self, state: RecState, lang: Lang) {
        self.update(|t| {
            t.state = state;
            t.lang = lang;
            t.message = None;
        });
    }

    fn set_language(&mut self, lang: Lang) {
        self.update(|t| t.lang = lang);
    }

    fn notify(&mut self, message: &str) {
        let sent = vixeeny_platform::notify::notify("Vixeeny", message).is_ok();
        if !sent {
            let message = message.to_string();
            self.update(|t| t.message = Some(message));
        }
    }
}

/// X11 key grabs, or the GlobalShortcuts portal on a Wayland session (grabs made through
/// XWayland only see keys pressed while an X11 window has the focus).
struct LinuxHotkeys {
    x11: Option<DesktopHotkeys>,
    portal: PortalShortcuts,
    wayland: bool,
    tx: EventTx,
}

impl HotkeyBackend for LinuxHotkeys {
    fn apply(&mut self, bindings: &[(ActionId, Hotkey)]) -> Vec<String> {
        if self.wayland {
            self.portal.apply(bindings, &self.tx);
            return Vec::new();
        }
        match &mut self.x11 {
            Some(x11) => x11.apply(bindings),
            None => vec!["no X11 display for the global shortcuts".into()],
        }
    }
}

pub fn run(startup: Startup) -> anyhow::Result<()> {
    let Startup {
        config,
        config_path,
        listener,
        link,
    } = startup;

    let (wake_tx, wake_rx) = channel();
    let (sender, receiver) = channel();
    let tx = EventTx::new(sender, Arc::new(ChannelWaker(wake_tx)));

    let os_locale = os_locale();
    let lang = Lang::resolve(&config.general.language, os_locale.as_deref());
    let tray = LinuxTray::new(lang, &tx);
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let x11 = match DesktopHotkeys::new(&tx) {
        Ok(hotkeys) => Some(hotkeys),
        Err(e) => {
            tracing::warn!("no X11 shortcuts: {e:#}");
            None
        }
    };
    let hotkeys = LinuxHotkeys {
        x11,
        portal: PortalShortcuts::default(),
        wayland,
        tx: tx.clone(),
    };
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

    // `recv` fails only if every sender is gone, which cannot happen while the runtime holds one.
    while wake_rx.recv().is_ok() {
        if runtime.pump() == Flow::Quit {
            break;
        }
    }
    Ok(())
}
