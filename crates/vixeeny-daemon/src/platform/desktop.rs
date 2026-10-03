// SPDX-License-Identifier: GPL-3.0-or-later
//! The parts of the daemon's platform layer that Windows and macOS share: the tray (menu bar)
//! icon with its menu, and the global shortcuts. Both crates hide the OS differences; only the
//! event loop and the wake-up differ (see `windows.rs`, `macos.rs`).

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use vixeeny_common::hotkey::Hotkey;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::{ActionId, RecState};

use crate::core::Event;
use crate::icon;
use crate::runtime::{HotkeyBackend, Tray};
use crate::server::EventTx;

pub struct DesktopTray {
    tray: TrayIcon,
    settings: MenuItem,
    quit: MenuItem,
}

impl DesktopTray {
    pub fn new(lang: Lang, tx: &EventTx) -> anyhow::Result<Self> {
        let settings = MenuItem::new(tr(Key::MenuSettings, lang), true, None);
        let quit = MenuItem::new(tr(Key::MenuQuit, lang), true, None);
        let menu = Menu::new();
        menu.append_items(&[&settings, &quit])
            .context("building the tray menu")?;

        let settings_id: MenuId = settings.id().clone();
        let quit_id: MenuId = quit.id().clone();
        let menu_tx = tx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == settings_id {
                menu_tx.send(Event::Action(ActionId::OpenSettings));
            } else if event.id == quit_id {
                menu_tx.send(Event::TrayQuit);
            }
        }));
        let click_tx = tx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                click_tx.send(Event::Action(ActionId::OpenSettings));
            }
        }));

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_icon(tray_image(false)?)
            .with_tooltip(tr(Key::TrayTooltip, lang))
            .build()
            .context("creating the tray icon")?;
        Ok(Self {
            tray,
            settings,
            quit,
        })
    }
}

fn tray_image(recording: bool) -> anyhow::Result<Icon> {
    Icon::from_rgba(icon::render(recording), icon::SIZE, icon::SIZE).context("building the icon")
}

impl Tray for DesktopTray {
    fn set_recording(&mut self, state: RecState, lang: Lang) {
        let recording = state == RecState::Recording;
        let key = if state == RecState::Idle {
            Key::TrayTooltip
        } else {
            Key::TrayTooltipRecording
        };
        match tray_image(recording) {
            Ok(image) => {
                if let Err(e) = self.tray.set_icon(Some(image)) {
                    tracing::warn!("cannot change the tray icon: {e}");
                }
            }
            Err(e) => tracing::warn!("{e:#}"),
        }
        if let Err(e) = self.tray.set_tooltip(Some(tr(key, lang))) {
            tracing::warn!("cannot change the tray tooltip: {e}");
        }
    }

    fn set_language(&mut self, lang: Lang) {
        self.settings.set_text(tr(Key::MenuSettings, lang));
        self.quit.set_text(tr(Key::MenuQuit, lang));
    }

    fn notify(&mut self, message: &str) {
        // Toast notifications arrive with M17; until then the tooltip is the only feedback.
        if let Err(e) = self.tray.set_tooltip(Some(message)) {
            tracing::warn!("cannot change the tray tooltip: {e}");
        }
    }
}

/// RegisterHotKey-based shortcuts: they fire while a game has the focus (CA-HK-2), except for
/// games that grab the keyboard exclusively. No hook, no polling: the shortcut arrives as a
/// message on the daemon's own loop.
pub struct DesktopHotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    actions: Arc<Mutex<HashMap<u32, ActionId>>>,
}

impl DesktopHotkeys {
    pub fn new(tx: &EventTx) -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new().context("creating the hotkey manager")?;
        let actions: Arc<Mutex<HashMap<u32, ActionId>>> = Arc::default();
        let (handler_actions, handler_tx) = (actions.clone(), tx.clone());
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() != HotKeyState::Pressed {
                return;
            }
            let action = handler_actions
                .lock()
                .ok()
                .and_then(|map| map.get(&event.id()).copied());
            if let Some(action) = action {
                handler_tx.send(Event::Action(action));
            }
        }));
        Ok(Self {
            manager,
            registered: Vec::new(),
            actions,
        })
    }
}

fn to_global(hotkey: &Hotkey) -> Result<HotKey, String> {
    let code = Code::from_str(&hotkey.code).map_err(|_| format!("unknown key {}", hotkey.code))?;
    let m = hotkey.mods;
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::CONTROL, m.ctrl);
    mods.set(Modifiers::ALT, m.alt);
    mods.set(Modifiers::SHIFT, m.shift);
    mods.set(Modifiers::SUPER, m.meta);
    Ok(HotKey::new(Some(mods), code))
}

impl HotkeyBackend for DesktopHotkeys {
    fn apply(&mut self, bindings: &[(ActionId, Hotkey)]) -> Vec<String> {
        // Forget the previous set first; a failure here only means it was not registered.
        let _ = self.manager.unregister_all(&self.registered);
        self.registered.clear();
        let Ok(mut actions) = self.actions.lock() else {
            return vec!["hotkey table poisoned".into()];
        };
        actions.clear();
        let mut failures = Vec::new();
        for (action, hotkey) in bindings {
            let result = to_global(hotkey).and_then(|global| {
                self.manager
                    .register(global)
                    .map(|()| global)
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(global) => {
                    actions.insert(global.id(), *action);
                    self.registered.push(global);
                }
                Err(e) => failures.push(format!("{hotkey} ({action:?}): {e}")),
            }
        }
        failures
    }
}
