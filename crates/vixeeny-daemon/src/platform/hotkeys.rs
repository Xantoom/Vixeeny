// SPDX-License-Identifier: GPL-3.0-or-later
//! Global shortcuts through `global-hotkey` (RegisterHotKey).

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use vixeeny_common::hotkey::Hotkey;
use vixeeny_common::ipc::ActionId;

use crate::core::Event;
use crate::runtime::HotkeyBackend;
use crate::server::EventTx;

/// RegisterHotKey-based shortcuts: they fire while a game has the focus (CA-HK-2), except for
/// games that grab the keyboard exclusively. No hook, no polling: the shortcut arrives as a
/// message on the daemon's own loop.
pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    actions: Arc<Mutex<HashMap<u32, ActionId>>>,
}

impl Hotkeys {
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

impl HotkeyBackend for Hotkeys {
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
