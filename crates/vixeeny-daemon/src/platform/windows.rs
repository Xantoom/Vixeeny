// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows: a message-only window owns the loop. Other threads wake it with `PostMessageW`
//! (not `PostThreadMessage`, which is dropped while a menu is open); the window procedure
//! drains the event queue, so events are handled even inside the tray menu's modal loop.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::channel;

use anyhow::{Context, bail};
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
use super::hotkeys::Hotkeys;
use super::tray::Tray;
use crate::runtime::{Flow, Runtime};
use crate::server::{self, EventTx, Waker};
use crate::supervisor::ProcessSpawner;

const WM_WAKE: u32 = WM_APP;
const LOCALE_NAME_MAX_LENGTH: usize = 85;

type DaemonRuntime = Runtime<Tray, ProcessSpawner>;

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
    let tray = Tray::new(lang, &tx)?;
    let hotkeys = Hotkeys::new(&tx)?;
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
