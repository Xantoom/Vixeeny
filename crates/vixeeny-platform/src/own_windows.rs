// SPDX-License-Identifier: GPL-3.0-or-later
//! Vixeeny's own floating windows (the notification cards, the side bar, the recording bar), whatever process shows
//! them. They are left out of screen captures unless the settings want them in: an image capture for the moment it
//! takes, a recording or the replay for as long as it runs. And they hold still while a zone capture shows the frozen
//! screens, which picture them as they were.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, TRUE, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetPropW, RegisterWindowMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetPropW,
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WINDOW_DISPLAY_AFFINITY,
};
use windows::core::{BOOL, PCWSTR, w};

/// The property that marks one of them.
const PROPERTY: PCWSTR = w!("Vixeeny.OwnWindow");
/// Set while they show in captures (a recording or the replay wants them): one for the user's session.
const STATE: PCWSTR = w!("Local\\Vixeeny.OwnWindowsCaptured");
/// A window that does not answer within this is left as it is.
const SEND_TIMEOUT_MS: u32 = 100;

fn registered(cell: &OnceLock<u32>, name: PCWSTR) -> u32 {
    // SAFETY: a constant, NUL-terminated name.
    *cell.get_or_init(|| unsafe { RegisterWindowMessageW(name) })
}

/// What one of them is told: `wparam` 1 shows it in captures, 0 leaves it out. [`handle`] does it.
pub fn capture_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    registered(&MESSAGE, w!("Vixeeny.OwnWindowCapture"))
}

/// What one of them is told: `wparam` 1 holds it still (a zone capture shows the frozen screens), 0 lets it go on.
pub fn hold_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    registered(&MESSAGE, w!("Vixeeny.OwnWindowHold"))
}

/// The event that says whether they show in captures, made by the first process that asks (kept while one has it).
fn state() -> Option<HANDLE> {
    static EVENT: OnceLock<Option<isize>> = OnceLock::new();
    let handle = EVENT.get_or_init(|| {
        // SAFETY: a manual-reset event of the session, created or opened; its handle is kept for the process's life.
        unsafe { CreateEventW(None, true, false, STATE) }.ok().map(|event| event.0 as isize)
    });
    handle.map(|h| HANDLE(h as *mut _))
}

/// Whether they show in captures now (a recording or the replay wants them).
pub fn captured() -> bool {
    // SAFETY: an event handle of this process; a zero wait only reads its state.
    state().is_some_and(|event| unsafe { WaitForSingleObject(event, 0) } == WAIT_OBJECT_0)
}

fn apply(hwnd: HWND, shown: bool) -> bool {
    let affinity = if shown { WINDOW_DISPLAY_AFFINITY(0) } else { WDA_EXCLUDEFROMCAPTURE };
    // SAFETY: a window of this process (a window procedure, or its creator, calls this).
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) }.is_ok()
}

/// Marks `hwnd`, a window of this process just made, as one of them: left out of captures unless they show in them now.
/// `false` when Windows cannot leave it out (before Windows 10 2004).
pub fn mark(hwnd: isize) -> bool {
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: a property on a window of this process; the value only marks it.
    let _ = unsafe { SetPropW(hwnd, PROPERTY, Some(HANDLE(std::ptr::without_provenance_mut(1)))) };
    apply(hwnd, captured())
}

/// For their window procedure: does what [`capture_message`] asks of window `hwnd`; `false` for any other message.
pub fn handle(hwnd: isize, msg: u32, wparam: usize) -> bool {
    if msg != capture_message() {
        return false;
    }
    apply(HWND(hwnd as *mut _), wparam != 0);
    true
}

/// Sends `msg` with `wparam` to every one of them, in any process; whether one of them is on screen.
fn send(msg: u32, wparam: usize) -> bool {
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    if unsafe { EnumWindows(Some(collect), LPARAM(&raw mut handles as isize)) }.is_err() {
        return false;
    }
    let mut on_screen = false;
    for hwnd in handles {
        // SAFETY: reading a property of a window; a window that went answers nothing.
        if unsafe { GetPropW(hwnd, PROPERTY) }.is_invalid() {
            continue;
        }
        on_screen |= crate::windows_impl::is_capturable(hwnd);
        // SAFETY: a registered message to a top-level window; one that hangs is skipped after the timeout.
        unsafe { SendMessageTimeoutW(hwnd, msg, WPARAM(wparam), LPARAM(0), SMTO_ABORTIFHUNG, SEND_TIMEOUT_MS, None) };
    }
    on_screen
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<HWND>` that `send` passes.
    unsafe { &mut *(data.0 as *mut Vec<HWND>) }.push(hwnd);
    TRUE
}

/// Runs `f`, a capture of the screen, with them in it (`shown`) or left out, then as they were. When one of them is on
/// screen, `f` waits for the compositor to show the change.
pub fn while_captured<T>(shown: bool, f: impl FnOnce() -> T) -> T {
    let before = captured();
    if shown == before {
        return f();
    }
    if send(capture_message(), usize::from(shown)) {
        crate::wait_for_composition();
    }
    let result = f();
    send(capture_message(), usize::from(before));
    result
}

/// Holds them still (a zone capture shows the frozen screens), or lets them go on.
pub fn hold(held: bool) {
    send(hold_message(), usize::from(held));
}

/// Holds them still while it lives: a zone capture shows the frozen screens.
pub struct Held(());

impl Held {
    pub fn new() -> Self {
        hold(true);
        Self(())
    }
}

impl Default for Held {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        hold(false);
    }
}

/// Shows them in captures, or leaves them out, from now on.
fn set_captured(shown: bool) {
    if let Some(event) = state() {
        // SAFETY: an event handle of this process.
        let _ = unsafe { if shown { SetEvent(event) } else { ResetEvent(event) } };
    }
    send(capture_message(), usize::from(shown));
}

/// Leaves them out of captures again whatever wanted them in: the process that recorded is gone.
pub fn reset() {
    if captured() {
        set_captured(false);
    }
    hold(false);
}

/// Held by a recording or the replay that wants them in its pictures: they show in captures while one is held.
pub struct InCaptures(());

static HELD: AtomicUsize = AtomicUsize::new(0);

impl InCaptures {
    pub fn new() -> Self {
        if HELD.fetch_add(1, Ordering::AcqRel) == 0 {
            set_captured(true);
        }
        Self(())
    }
}

impl Default for InCaptures {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for InCaptures {
    fn drop(&mut self) {
        if HELD.fetch_sub(1, Ordering::AcqRel) == 1 {
            set_captured(false);
        }
    }
}
