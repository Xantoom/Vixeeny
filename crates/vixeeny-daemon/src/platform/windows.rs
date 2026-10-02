// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows: a message-only window owns the loop. Other threads wake it with `PostMessageW`
//! (not `PostThreadMessage`, which is dropped while a menu is open); the window procedure
//! drains the event queue, so events are handled even inside the tray menu's modal loop.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::channel;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use anyhow::{Context, bail};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use vixeeny_common::hotkey::Hotkey;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::{ActionId, RecState};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    PostMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WNDCLASSEXW,
};
use windows::core::w;

use super::Startup;
use crate::core::Event;
use crate::icon;
use crate::runtime::{Flow, HotkeyBackend, Runtime, Tray};
use crate::server::{self, EventTx, Waker};
use crate::supervisor::ProcessSpawner;

const WM_WAKE: u32 = WM_APP;
const LOCALE_NAME_MAX_LENGTH: usize = 85;

type DaemonRuntime = Runtime<WinTray, ProcessSpawner>;

thread_local! {
    static RUNTIME: RefCell<Option<DaemonRuntime>> = const { RefCell::new(None) };
}

/// Posts [`WM_WAKE`] to the message-only window. `HWND` is not `Send`, so keep its value.
struct WindowWaker {
    hwnd: AtomicIsize,
}

impl Waker for WindowWaker {
    fn wake(&self) {
        let hwnd = HWND(self.hwnd.load(Ordering::Relaxed) as *mut _);
        // SAFETY: PostMessageW may be called from any thread. If the window is already
        // destroyed the call just fails, which is fine during shutdown.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_WAKE, WPARAM(0), LPARAM(0)) };
    }
}

pub struct WinTray {
    tray: TrayIcon,
    settings: MenuItem,
    quit: MenuItem,
}

impl WinTray {
    fn new(lang: Lang, tx: &EventTx) -> anyhow::Result<Self> {
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

impl Tray for WinTray {
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
pub struct WinHotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    actions: Arc<Mutex<HashMap<u32, ActionId>>>,
}

impl WinHotkeys {
    fn new(tx: &EventTx) -> anyhow::Result<Self> {
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

impl HotkeyBackend for WinHotkeys {
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

fn os_locale() -> Option<String> {
    let mut buf = [0u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: the buffer is valid for writes and its length is passed to the call.
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    let len = usize::try_from(len).ok()?.checked_sub(1)?; // drop the NUL
    String::from_utf16(buf.get(..len)?).ok()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_WAKE {
        // `try_borrow_mut`: a re-entrant wake (from inside the runtime) is picked up by the
        // pump that is already running.
        let flow = RUNTIME.with(|cell| {
            cell.try_borrow_mut()
                .ok()
                .and_then(|mut rt| rt.as_mut().map(Runtime::pump))
        });
        if flow == Some(Flow::Quit) {
            // SAFETY: plain call, no pointers involved.
            unsafe { PostQuitMessage(0) };
        }
        return LRESULT(0);
    }
    // SAFETY: forwards the arguments we received, unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_window() -> anyhow::Result<HWND> {
    // SAFETY: standard window-class registration and window creation with valid arguments; the
    // class name is a static string and the window procedure has the required signature.
    unsafe {
        let instance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let class = w!("VixeenyDaemonWindow");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            bail!("RegisterClassExW failed");
        }
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Vixeeny"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .context("CreateWindowExW")
    }
}

pub fn run(startup: Startup) -> anyhow::Result<()> {
    let Startup {
        config,
        config_path,
        listener,
        link,
    } = startup;

    let hwnd = create_window()?;
    let waker = Arc::new(WindowWaker {
        hwnd: AtomicIsize::new(hwnd.0 as isize),
    });
    let (sender, receiver) = channel();
    let tx = EventTx::new(sender, waker);

    let os_locale = os_locale();
    let lang = vixeeny_common::i18n::Lang::resolve(&config.general.language, os_locale.as_deref());
    let tray = WinTray::new(lang, &tx)?;
    let hotkeys = WinHotkeys::new(&tx)?;
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

    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG for the duration of the loop; GetMessageW blocks until a
    // message arrives (no polling) and returns 0 on WM_QUIT, -1 on error.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // Drop the runtime (and the tray icon) before exiting so the icon disappears at once.
    RUNTIME.with(|cell| cell.borrow_mut().take());
    Ok(())
}
