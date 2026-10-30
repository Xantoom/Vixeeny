// SPDX-License-Identifier: GPL-3.0-or-later
//! The native windows: borderless, top-most popups without a redirection bitmap (their only content is a
//! DirectComposition tree), created cloaked so they appear with their first content. The editor has its own class; the
//! popups (side strip, notification, recording widget) share another, and a message-only window carries the messages
//! other threads post.

use std::sync::OnceLock;

use vixeeny_editor::CursorHint;
use vixeeny_platform::PhysicalRect;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, HCURSOR, HWND_MESSAGE, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW,
    IDC_CROSS, IDC_HAND, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, LoadCursorW,
    RegisterClassExW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowDisplayAffinity, SetWindowPos,
    WDA_EXCLUDEFROMCAPTURE, WINDOW_DISPLAY_AFFINITY, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSEXW, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, Result, w};

const CLASS: PCWSTR = w!("VixeenyEditor");

/// The window procedure of every editor window.
pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

fn instance() -> HINSTANCE {
    // SAFETY: plain call; `None` asks for this executable's module.
    unsafe { GetModuleHandleW(None) }.map(Into::into).unwrap_or_default()
}

/// Registers a window class once per process.
fn register(class: PCWSTR, proc: WndProc, done: &'static OnceLock<bool>) -> bool {
    *done.get_or_init(|| {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: instance(),
            // The cursor is set on every move (WM_SETCURSOR).
            hCursor: HCURSOR::default(),
            lpszClassName: class,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised and its strings are static.
        unsafe { RegisterClassExW(&class) != 0 }
    })
}

/// A cloaked window of `class` over `rect`.
fn new_window(
    class: PCWSTR,
    ex: WINDOW_EX_STYLE,
    style: WINDOW_STYLE,
    title: PCWSTR,
    rect: PhysicalRect,
) -> Result<HWND> {
    // SAFETY: the class is registered; the title outlives the call.
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            class,
            title,
            style,
            rect.x,
            rect.y,
            i32::try_from(rect.width).unwrap_or(i32::MAX),
            i32::try_from(rect.height).unwrap_or(i32::MAX),
            None,
            None,
            Some(instance()),
            None,
        )
    }?;
    set_cloak(hwnd, true);
    Ok(hwnd)
}

fn popup_window(class: PCWSTR, ex: WINDOW_EX_STYLE, rect: PhysicalRect) -> Result<HWND> {
    let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP | ex;
    let hwnd = new_window(class, ex, WS_POPUP, w!("Vixeeny"), rect)?;
    let off = BOOL::from(true);
    // SAFETY: `off` outlives the call, which sets an attribute of the window just created.
    let _ = unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, (&raw const off).cast(), size_of::<BOOL>() as u32)
    };
    Ok(hwnd)
}

/// A cloaked, hidden editor window over `(x, y, w, h)` (physical pixels).
pub fn create(proc: WndProc, x: i32, y: i32, w: u32, h: u32) -> Result<HWND> {
    static DONE: OnceLock<bool> = OnceLock::new();
    if !register(CLASS, proc, &DONE) {
        return Err(windows::core::Error::from_thread());
    }
    popup_window(CLASS, WINDOW_EX_STYLE::default(), PhysicalRect::new(x, y, w, h))
}

const POPUP_CLASS: PCWSTR = w!("VixeenyPopup");
static POPUP_DONE: OnceLock<bool> = OnceLock::new();

/// Whether a popup takes the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// When shown or clicked.
    Take,
    /// Never, even when clicked.
    Never,
}

/// Whether a popup shows in screen captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    Shown,
    /// Left out of every screen capture from its creation (Windows 10 2004 or later).
    Hidden,
    /// One of Vixeeny's floating windows (a notification card, the side bar, the recording bar): left out of screen
    /// captures unless the settings want them in (see [`vixeeny_platform::own_windows`]).
    Own,
}

/// A cloaked, hidden popup over `geometry` (physical pixels).
pub fn create_popup(proc: WndProc, geometry: PhysicalRect, focus: Focus, capture: Capture) -> Result<HWND> {
    if !register(POPUP_CLASS, proc, &POPUP_DONE) {
        return Err(windows::core::Error::from_thread());
    }
    let ex = match focus {
        Focus::Take => WINDOW_EX_STYLE::default(),
        Focus::Never => WS_EX_NOACTIVATE,
    };
    let hwnd = popup_window(POPUP_CLASS, ex, geometry)?;
    match capture {
        Capture::Shown => {}
        Capture::Hidden => {
            // SAFETY: plain call on the window just created.
            if let Err(e) = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } {
                tracing::error!("a window cannot be excluded from the capture: {e}");
            }
        }
        Capture::Own => {
            if !vixeeny_platform::own_windows::mark(hwnd.0 as isize) {
                tracing::error!("a window cannot be excluded from the capture");
            }
        }
    }
    Ok(hwnd)
}

/// A cloaked, hidden window of its own over `rect` (physical pixels): in the taskbar and Alt+Tab, under the windows
/// that come after it, with rounded corners on Windows 11.
pub fn create_app_window(proc: WndProc, rect: PhysicalRect, title: &str) -> Result<HWND> {
    use windows::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
        SendMessageW, WM_SETICON, WS_EX_APPWINDOW, WS_MINIMIZEBOX, WS_SYSMENU,
    };
    if !register(POPUP_CLASS, proc, &POPUP_DONE) {
        return Err(windows::core::Error::from_thread());
    }
    let title = windows::core::HSTRING::from(title);
    let hwnd = new_window(
        POPUP_CLASS,
        WS_EX_APPWINDOW | WS_EX_NOREDIRECTIONBITMAP,
        WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX,
        PCWSTR(title.as_ptr()),
        rect,
    )?;
    // SAFETY: attributes are set on the window just created.
    unsafe {
        // The executable's icon, for the taskbar and Alt+Tab (the class has none).
        for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
            let size = GetSystemMetrics(metric);
            // MAKEINTRESOURCE(1).
            let name = PCWSTR(std::ptr::without_provenance(1));
            if let Ok(icon) = LoadImageW(Some(instance()), name, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
        // DWMWCP_ROUND.
        let round = 2_i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const round).cast(),
            size_of::<i32>() as u32,
        );
        Ok(hwnd)
    }
}

/// A message-only window, for the messages other threads post.
pub fn create_message_window(proc: WndProc) -> Result<HWND> {
    if !register(POPUP_CLASS, proc, &POPUP_DONE) {
        return Err(windows::core::Error::from_thread());
    }
    // SAFETY: the class is registered; the strings are static.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            POPUP_CLASS,
            w!("Vixeeny"),
            Default::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance()),
            None,
        )
    }
}

/// Hides (or shows) the window from the screen without changing its state otherwise.
pub fn set_cloak(hwnd: HWND, on: bool) {
    let value = BOOL::from(on);
    // SAFETY: plain call on a window of this thread.
    let _ = unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, (&raw const value).cast(), size_of::<BOOL>() as u32) };
}

/// Leaves the window out of screen captures (`on`), or puts it back; `false` when Windows cannot (before Windows 10
/// 2004).
pub fn set_excluded(hwnd: HWND, on: bool) -> bool {
    let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WINDOW_DISPLAY_AFFINITY(0) };
    // SAFETY: plain call on a window of this thread.
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) }.is_ok()
}

/// Keeps the window above every other one, or lets a dialog come above it.
pub fn set_topmost(hwnd: HWND, on: bool) {
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    // SAFETY: plain call on a window of this thread.
    let _ = unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
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
