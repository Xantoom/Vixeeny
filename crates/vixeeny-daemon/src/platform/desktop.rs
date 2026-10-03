// SPDX-License-Identifier: GPL-3.0-or-later
//! The tray (menu bar) icon with its menu, shared by Windows and macOS. The crate hides the OS
//! differences; only the event loop and the wake-up differ (see `windows.rs`, `macos.rs`).

use anyhow::Context;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::{ActionId, RecState};

use crate::core::Event;
use crate::icon;
use crate::runtime::Tray;
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
