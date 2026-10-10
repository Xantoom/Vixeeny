// SPDX-License-Identifier: GPL-3.0-or-later
//! What the popups (side strip, notification, recording widget) share: a borderless window whose
//! only content is a DirectComposition tree, the routing of its messages, a mailbox for the
//! messages of other threads, and the message loop.
//!
//! Nothing presents frames the way a game does: a popup draws into its surfaces when something
//! changes and the compositor runs its motion. A screen with a variable refresh rate keeps its
//! rate while one is up.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::DirectComposition::{IDCompositionTarget, IDCompositionVisual2};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetMessageW, MA_NOACTIVATE, MSG, PostMessageW, SW_SHOW,
    SW_SHOWNOACTIVATE, SetCursor, SetForegroundWindow, ShowWindow, TranslateMessage, WM_APP,
    WM_DPICHANGED, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_PAINT,
};
use windows::core::Result;

use crate::gfx::Gfx;
use crate::scene;
use crate::window::{self, Pointer};

/// What a popup does with a message; `None` leaves it to Windows.
type Handler = Rc<dyn Fn(u32, WPARAM, LPARAM) -> Option<LRESULT>>;

thread_local! {
    static HANDLERS: RefCell<HashMap<isize, Handler>> = RefCell::default();
}

/// `WM_MOUSELEAVE` (declared with the common controls in the bindings).
pub const WM_MOUSELEAVE: u32 = 0x02a3;
/// Mail for a [`Mailbox`].
const WM_MAIL: u32 = WM_APP + 1;
/// Free for the popups' own use.
pub const WM_POPUP: u32 = WM_APP + 2;

fn key(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Cloned out of the map: a handler may open or close popups.
    let handler = HANDLERS.with(|h| h.borrow().get(&key(hwnd)).cloned());
    if let Some(handler) = handler
        && let Some(result) = handler(msg, wparam, lparam)
    {
        return result;
    }
    match msg {
        WM_PAINT => {
            // SAFETY: plain call; the content is DirectComposition's, nothing to paint.
            let _ = unsafe { ValidateRect(Some(hwnd), None) };
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        // The window keeps the size its host gave, in physical pixels.
        WM_DPICHANGED => LRESULT(0),
        _ => window::default_proc(hwnd, msg, wparam, lparam),
    }
}

/// The low and high signed words of a message parameter (pointer coordinates).
pub fn point(lparam: LPARAM) -> (f32, f32) {
    let v = lparam.0;
    (
        f32::from(v as u16 as i16),
        f32::from((v >> 16) as u16 as i16),
    )
}

/// One popup window and its visual tree.
pub struct Popup {
    pub hwnd: HWND,
    _target: IDCompositionTarget,
    pub root: IDCompositionVisual2,
    activate: bool,
}

impl Popup {
    /// A hidden window over `geometry` (physical pixels); see [`window::create_popup`].
    pub fn new(
        gfx: &Gfx,
        geometry: (i32, i32, u32, u32),
        activate: bool,
        hidden_from_capture: bool,
    ) -> Result<Self> {
        let hwnd = window::create_popup(wndproc, geometry, activate, hidden_from_capture)?;
        // SAFETY: plain creation calls for a window of this thread.
        let made = unsafe {
            gfx.dcomp
                .CreateTargetForHwnd(hwnd, true)
                .and_then(|target| {
                    let root = scene::visual(gfx)?;
                    target.SetRoot(&root)?;
                    Ok((target, root))
                })
        };
        match made {
            Ok((target, root)) => Ok(Self {
                hwnd,
                _target: target,
                root,
                activate,
            }),
            Err(e) => {
                // SAFETY: the window was created above by this thread.
                let _ = unsafe { DestroyWindow(hwnd) };
                Err(e)
            }
        }
    }

    /// Routes the window's messages to `handler`.
    pub fn on_message(&self, handler: impl Fn(u32, WPARAM, LPARAM) -> Option<LRESULT> + 'static) {
        let handler: Handler = Rc::new(handler);
        HANDLERS.with(|h| h.borrow_mut().insert(key(self.hwnd), handler));
    }

    /// Shows the window once what was committed so far is on the compositor: it appears with
    /// its content. One that activates takes the keyboard.
    pub fn show(&self, gfx: &Gfx) -> Result<()> {
        // SAFETY: plain calls on a window of this thread.
        unsafe {
            gfx.dcomp.WaitForCommitCompletion()?;
            if self.activate {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(Some(self.hwnd));
            } else {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        window::set_cloak(self.hwnd, false);
        // SAFETY: plain call: waits for the composition that shows the window.
        let _ = unsafe { DwmFlush() };
        Ok(())
    }

    /// Hides the window at once (it stays until dropped).
    pub fn hide(&self) {
        window::set_cloak(self.hwnd, true);
    }

    /// Asks for a `WM_MOUSELEAVE` when the pointer leaves the window.
    pub fn track_leave(&self) {
        let mut t = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        // SAFETY: `t` is valid for the call.
        let _ = unsafe { TrackMouseEvent(&mut t) };
    }

    /// The pointer is over the window.
    pub fn has_pointer(&self) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
        let (mut at, mut r) = (POINT::default(), RECT::default());
        // SAFETY: valid out-pointers, a window of this thread.
        unsafe {
            GetCursorPos(&mut at).is_ok()
                && GetWindowRect(self.hwnd, &mut r).is_ok()
                && (r.left..r.right).contains(&at.x)
                && (r.top..r.bottom).contains(&at.y)
        }
    }

    /// Posts [`WM_POPUP`] with `wparam` to the window, to be handled on a later turn of the loop.
    pub fn post(&self, wparam: usize) {
        // SAFETY: plain call on a window of this thread.
        let _ = unsafe { PostMessageW(Some(self.hwnd), WM_POPUP, WPARAM(wparam), LPARAM(0)) };
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        HANDLERS.with(|h| h.borrow_mut().remove(&key(self.hwnd)));
        // SAFETY: the window was created by this thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// What the popups answer to `WM_MOUSEACTIVATE` when they must not take the keyboard.
pub const NO_ACTIVATE: LRESULT = LRESULT(MA_NOACTIVATE as isize);

/// The answer to `msg` common to the popups that never take the keyboard, if any.
pub fn passive(msg: u32) -> Option<LRESULT> {
    (msg == WM_MOUSEACTIVATE).then_some(NO_ACTIVATE)
}

/// The part of the monitor nearest to `p` that the taskbar and the docked bars leave free.
pub fn work_area_at(p: POINT) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: plain calls; `info` is a valid, sized out-pointer.
    unsafe {
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info)
            .as_bool()
            .then_some(info.rcWork)
    }
}

/// The top-left corner that keeps a `size` window at `at` inside `area` (pinned to its top-left
/// corner when larger).
pub fn keep_inside(at: (i32, i32), size: (i32, i32), area: RECT) -> (i32, i32) {
    let fit = |v: i32, len: i32, lo: i32, hi: i32| v.min(hi - len).max(lo);
    (
        fit(at.0, size.0, area.left, area.right),
        fit(at.1, size.1, area.top, area.bottom),
    )
}

/// Shows the system's context menu of `items` under the pointer, for the window `owner`, the
/// last item set apart (a "Close"), in the dark or light theme. Returns the chosen item. The
/// window that had the keyboard gets it back unless an item was chosen.
pub fn context_menu(owner: HWND, items: &[String], dark: bool) -> Option<usize> {
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetForegroundWindow, MF_SEPARATOR,
        MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
    };
    menu_theme(dark);
    // SAFETY: plain menu calls; the menu is destroyed before returning, the strings outlive the
    // calls that read them.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let labels: Vec<windows::core::HSTRING> = items.iter().map(|i| i.into()).collect();
        for (i, label) in labels.iter().enumerate() {
            if i + 1 == labels.len() && i > 0 {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            }
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                i + 1,
                windows::core::PCWSTR(label.as_ptr()),
            );
        }
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // The menu closes on a click elsewhere only when its window is in the foreground.
        let before = GetForegroundWindow();
        let _ = SetForegroundWindow(owner);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            at.x,
            at.y,
            None,
            owner,
            None,
        )
        .0 as usize;
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if chosen == 0 && !before.is_invalid() {
            let _ = SetForegroundWindow(before);
        }
        chosen.checked_sub(1)
    }
}

/// Puts the menus of this process in the dark or light theme. Windows only exposes this through
/// two unnamed functions of uxtheme (`SetPreferredAppMode`, `FlushMenuThemes`, Windows 10 1903
/// and later); without them the menus stay light.
fn menu_theme(dark: bool) {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type SetMode = unsafe extern "system" fn(i32) -> i32;
    type Flush = unsafe extern "system" fn();
    // SAFETY: uxtheme stays loaded; the ordinals have had these signatures since 1903.
    unsafe {
        let Ok(lib) = LoadLibraryW(windows::core::w!("uxtheme.dll")) else {
            return;
        };
        let set = GetProcAddress(lib, windows::core::PCSTR(135 as *const u8));
        let flush = GetProcAddress(lib, windows::core::PCSTR(136 as *const u8));
        if let (Some(set), Some(flush)) = (set, flush) {
            let set: SetMode = std::mem::transmute(set);
            let flush: Flush = std::mem::transmute(flush);
            // 2: force dark, 3: force light.
            set(if dark { 2 } else { 3 });
            flush();
        }
    }
}

pub fn set_pointer(p: Pointer) {
    // SAFETY: plain call with a system cursor.
    unsafe { SetCursor(Some(window::cursor(p))) };
}

/// The receiving end of a mailbox: a message-only window of this thread. Mail sent from any
/// thread is handed to its callback here, on a turn of the message loop.
pub struct Mailbox {
    hwnd: HWND,
}

impl Drop for Mailbox {
    fn drop(&mut self) {
        HANDLERS.with(|h| h.borrow_mut().remove(&key(self.hwnd)));
        // SAFETY: the window was created by this thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// The sending end of a mailbox, for any thread.
pub struct Sender<T> {
    tx: mpsc::Sender<T>,
    hwnd: isize,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            hwnd: self.hwnd,
        }
    }
}

impl<T: Send> Sender<T> {
    /// Sends `mail`; `false` once the mailbox is gone.
    pub fn send(&self, mail: T) -> bool {
        if self.tx.send(mail).is_err() {
            return false;
        }
        // SAFETY: plain call; a window that is gone makes it fail, nothing else.
        unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_MAIL,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .is_ok()
    }
}

/// A mailbox whose mail goes to `on_mail` on this thread.
pub fn mailbox<T: Send + 'static>(on_mail: impl Fn(T) + 'static) -> Result<(Mailbox, Sender<T>)> {
    let hwnd = window::create_message_window(wndproc)?;
    let (tx, rx) = mpsc::channel::<T>();
    let handler: Handler = Rc::new(move |msg, _, _| {
        if msg != WM_MAIL {
            return None;
        }
        while let Ok(mail) = rx.try_recv() {
            on_mail(mail);
        }
        Some(LRESULT(0))
    });
    HANDLERS.with(|h| h.borrow_mut().insert(key(hwnd), handler));
    Ok((
        Mailbox { hwnd },
        Sender {
            tx,
            hwnd: key(hwnd),
        },
    ))
}

/// Runs the message loop of this thread while `keep` says so (asked after every message).
pub fn pump_while(keep: impl Fn() -> bool) {
    let mut msg = MSG::default();
    while keep() {
        // SAFETY: `msg` is valid for the calls; messages of this thread.
        unsafe {
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
