// SPDX-License-Identifier: GPL-3.0-or-later
//! The editor's windows: borderless, top-most popups without a redirection bitmap (their only
//! content is a DirectComposition tree), created cloaked so they appear with their first content.

use std::sync::OnceLock;

use vixeeny_editor::CursorHint;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, HCURSOR, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW,
    IDC_CROSS, IDC_HAND, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
    LoadCursorW, RegisterClassExW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    WNDCLASSEXW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("VixeenyEditor");

/// The window procedure of every editor window.
pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

fn instance() -> HINSTANCE {
    // SAFETY: plain call; `None` asks for this executable's module.
    unsafe { GetModuleHandleW(None) }
        .map(Into::into)
        .unwrap_or_default()
}

/// Registers the window class once per process.
fn register(proc: WndProc) -> bool {
    static DONE: OnceLock<bool> = OnceLock::new();
    *DONE.get_or_init(|| {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: instance(),
            // The cursor is set on every move (WM_SETCURSOR).
            hCursor: HCURSOR::default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised and its strings are static.
        unsafe { RegisterClassExW(&class) != 0 }
    })
}

/// A cloaked, hidden editor window over `(x, y, w, h)` (physical pixels).
pub fn create(proc: WndProc, x: i32, y: i32, w: u32, h: u32) -> Result<HWND> {
    if !register(proc) {
        return Err(windows::core::Error::from_thread());
    }
    // SAFETY: the class is registered; the strings are static. Attributes are set on the window
    // just created.
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            w!("Vixeeny"),
            WS_POPUP,
            x,
            y,
            i32::try_from(w).unwrap_or(i32::MAX),
            i32::try_from(h).unwrap_or(i32::MAX),
            None,
            None,
            Some(instance()),
            None,
        )?;
        set_cloak(hwnd, true);
        let off = BOOL::from(true);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            (&raw const off).cast(),
            size_of::<BOOL>() as u32,
        );
        Ok(hwnd)
    }
}

/// Hides (or shows) the window from the screen without changing its state otherwise.
pub fn set_cloak(hwnd: HWND, on: bool) {
    let value = BOOL::from(on);
    // SAFETY: plain call on a window of this thread.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            (&raw const value).cast(),
            size_of::<BOOL>() as u32,
        )
    };
}

/// Keeps the window above every other one, or lets a dialog come above it.
pub fn set_topmost(hwnd: HWND, on: bool) {
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    // SAFETY: plain call on a window of this thread.
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
}

pub fn default_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: forwards the message as received.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// What the pointer looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    Zone(CursorHint),
    /// Over a button.
    Hand,
    Arrow,
}

pub fn cursor(p: Pointer) -> HCURSOR {
    let id = match p {
        Pointer::Hand => IDC_HAND,
        Pointer::Arrow | Pointer::Zone(CursorHint::Default) => IDC_ARROW,
        Pointer::Zone(CursorHint::Crosshair) => IDC_CROSS,
        Pointer::Zone(CursorHint::Move) => IDC_SIZEALL,
        Pointer::Zone(CursorHint::ResizeNs) => IDC_SIZENS,
        Pointer::Zone(CursorHint::ResizeEw) => IDC_SIZEWE,
        Pointer::Zone(CursorHint::ResizeNeSw) => IDC_SIZENESW,
        Pointer::Zone(CursorHint::ResizeNwSe) => IDC_SIZENWSE,
    };
    // SAFETY: a system cursor, no module.
    unsafe { LoadCursorW(None, id) }.unwrap_or_default()
}
