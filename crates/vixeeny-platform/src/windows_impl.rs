// SPDX-License-Identifier: GPL-3.0-or-later
//! Win32 implementation: `EnumDisplayMonitors`, per-monitor DPI, DWM frame bounds.

use std::ffi::c_void;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWINDOWATTRIBUTE, DwmGetWindowAttribute,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIDevice, IDXGIFactory1, IDXGIOutput6,
};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, EnumWindows, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetWindowLongW,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, HHOOK, IsWindowVisible,
    SetWindowsHookExW, UnhookWindowsHookEx, WS_EX_TOOLWINDOW,
};
use windows::core::{Interface, PWSTR};

use crate::{
    HdrInfo, MonitorId, MonitorInfo, PhysicalRect, PlatformError, Result, WindowId, WindowInfo,
};

const MONITORINFOF_PRIMARY: u32 = 1;

/// Opts the process into per-monitor DPI awareness (v2) so every API reports physical pixels.
/// Failure means it was already set (manifest or earlier call), which is fine.
pub fn ensure_dpi_aware() {
    // SAFETY: plain call with a constant context handle.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

fn rect_of(r: RECT) -> PhysicalRect {
    PhysicalRect::new(
        r.left,
        r.top,
        u32::try_from(r.right - r.left).unwrap_or(0),
        u32::try_from(r.bottom - r.top).unwrap_or(0),
    )
}

fn os_err(what: &str, e: windows::core::Error) -> PlatformError {
    PlatformError::Os(format!("{what}: {e}"))
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed by `monitors()` below, alive for the
    // whole (synchronous) enumeration.
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(monitor);
    TRUE
}

pub fn monitors() -> Result<Vec<MonitorInfo>> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only dereferences the pointer to `handles`, which outlives the call.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&raw mut handles as isize),
        )
    };
    if !ok.as_bool() {
        return Err(PlatformError::Os("EnumDisplayMonitors failed".into()));
    }
    let mut out = Vec::with_capacity(handles.len());
    for handle in handles {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `info` is a valid MONITORINFOEXW whose size field is set; MONITORINFOEXW
        // starts with a MONITORINFO, so the pointer cast is the documented usage.
        let ok = unsafe { GetMonitorInfoW(handle, (&raw mut info).cast::<MONITORINFO>()) };
        if !ok.as_bool() {
            continue; // unplugged between the two calls
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        // SAFETY: both out-pointers are valid for the call. On failure keep 96 (100 %).
        let _ = unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        let name_len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        out.push(MonitorInfo {
            id: MonitorId(handle.0 as usize as u64),
            name: String::from_utf16_lossy(&info.szDevice[..name_len]),
            rect: rect_of(info.monitorInfo.rcMonitor),
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            dpi: dpi_x,
            hdr: hdr_info(MonitorId(handle.0 as usize as u64)),
        });
    }
    Ok(out)
}

/// The highest refresh rate among the monitors, in Hz (60 when it cannot be read).
pub fn max_refresh_hz() -> u32 {
    let Ok(list) = monitors() else { return 60 };
    list.iter()
        .filter_map(|m| {
            let name: Vec<u16> = m.name.encode_utf16().chain([0]).collect();
            let mut mode = DEVMODEW {
                dmSize: size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            // SAFETY: `name` is NUL-terminated and `mode` a DEVMODEW whose size field is set.
            let ok = unsafe {
                EnumDisplaySettingsW(
                    windows::core::PCWSTR(name.as_ptr()),
                    ENUM_CURRENT_SETTINGS,
                    &raw mut mode,
                )
            };
            // 0 and 1 mean "the hardware default".
            (ok.as_bool() && mode.dmDisplayFrequency > 1).then_some(mode.dmDisplayFrequency)
        })
        .max()
        .unwrap_or(60)
}

/// Whether Windows shows HDR ("Use HDR" on) on at least one monitor.
pub fn hdr_active() -> bool {
    monitors().is_ok_and(|list| list.iter().any(|m| m.hdr.is_some()))
}

pub fn cursor_position() -> Result<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out-pointer.
    unsafe { GetCursorPos(&mut p) }.map_err(|e| os_err("GetCursorPos", e))?;
    Ok((p.x, p.y))
}

fn hwnd_of(id: WindowId) -> HWND {
    HWND(id.0 as usize as *mut c_void)
}

pub fn foreground_window() -> Result<Option<WindowInfo>> {
    // SAFETY: no arguments; returns NULL when there is no foreground window.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return Ok(None);
    }
    window_info(WindowId(hwnd.0 as usize as u64))
}

/// Visible frame of the window. DWM excludes the invisible resize border that
/// `GetWindowRect` includes on Windows 10/11.
fn frame_rect(hwnd: HWND) -> Option<PhysicalRect> {
    let mut r = RECT::default();
    // SAFETY: `r` is a RECT and the size passed matches it.
    let dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut r).cast::<c_void>(),
            size_of::<RECT>() as u32,
        )
    };
    if dwm.is_err() {
        // SAFETY: `r` is a valid out-pointer.
        unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    }
    Some(rect_of(r))
}

/// The full path of the executable of process `pid`, when it can be read.
pub fn process_path(pid: u32) -> Option<String> {
    exe_path(pid)
}

/// Whether the process `pid` is still running.
pub fn process_alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::GetExitCodeProcess;
    /// What `GetExitCodeProcess` reports while the process runs.
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: the handle is closed on every path; `code` is a valid out-pointer.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(process, &raw mut code).is_ok();
        let _ = CloseHandle(process);
        ok && code == STILL_ACTIVE
    }
}

fn exe_path(pid: u32) -> Option<String> {
    // SAFETY: standard query of a process image name into a buffer whose capacity is passed
    // and updated by the call; the handle is closed on every path.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn window_info(id: WindowId) -> Result<Option<WindowInfo>> {
    let hwnd = hwnd_of(id);
    let Some(rect) = frame_rect(hwnd) else {
        return Ok(None); // the window is gone
    };
    let mut title = [0u16; 512];
    // SAFETY: the buffer is valid for writes and its length is passed.
    let len = unsafe { GetWindowTextW(hwnd, &mut title) };
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out-pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    Ok(Some(WindowInfo {
        id,
        title: String::from_utf16_lossy(&title[..usize::try_from(len).unwrap_or(0)]),
        rect,
        pid,
        exe_path: exe_path(pid),
    }))
}

unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
    // SAFETY: `data` is the `&mut Vec<HWND>` passed by the `EnumWindows` callers of this module.
    let list = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    TRUE
}

fn is_capturable(hwnd: HWND) -> bool {
    // SAFETY: simple queries on a window handle; stale handles just return false/errors.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        // Windows on other virtual desktops and suspended UWP apps are "cloaked".
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast::<c_void>(),
            size_of::<u32>() as u32,
        );
        cloaked == 0
    }
}

/// Visible, titled top-level windows, front to back (the order of `EnumWindows`).
pub fn top_level_windows() -> Result<Vec<WindowInfo>> {
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only dereferences the pointer to `handles`, which outlives the call.
    unsafe { EnumWindows(Some(collect_window), LPARAM(&raw mut handles as isize)) }
        .map_err(|e| os_err("EnumWindows", e))?;
    let mut out = Vec::new();
    for hwnd in handles.into_iter().filter(|h| is_capturable(*h)) {
        if let Some(info) = window_info(WindowId(hwnd.0 as usize as u64))?
            && !info.title.is_empty()
            && !info.rect.is_empty()
        {
            out.push(info);
        }
    }
    Ok(out)
}

/// `true` when the user chose the dark theme for apps (`AppsUseLightTheme` = 0).
pub fn system_prefers_dark() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut data = 1_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `data` and `size` are valid for the call; a missing value leaves `data` at 1.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&raw mut size),
        )
    };
    status.is_ok() && data == 0
}

/// `false` when the user turned off animations in Windows ("Show animations in Windows").
pub fn animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut on = TRUE;
    // SAFETY: `on` is a valid BOOL for the call.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on.as_bool()
}

/// Waits until the compositor has drawn the next frame of the screen.
pub fn wait_for_composition() {
    // SAFETY: plain call.
    let _ = unsafe { windows::Win32::Graphics::Dwm::DwmFlush() };
}

/// The window and the callback of [`on_click_outside`].
type Outside = Option<(HWND, Box<dyn Fn()>)>;

thread_local! {
    static OUTSIDE: std::cell::RefCell<Outside> = const { std::cell::RefCell::new(None) };
}

/// Removes the hook of [`on_click_outside`] when dropped.
pub struct OutsideClicks(HHOOK);

impl Drop for OutsideClicks {
    fn drop(&mut self) {
        // SAFETY: the hook was installed by `on_click_outside` and is removed once.
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
        OUTSIDE.with(|o| o.borrow_mut().take());
    }
}

/// Calls `clicked` whenever a mouse button goes down outside window `id`, wherever the focus
/// is. Losing the focus is not enough to notice a click elsewhere: a window Windows did not let
/// come to the front never had it. The calling thread must pump messages (any UI event loop
/// does); `clicked` runs on it.
pub fn on_click_outside(id: WindowId, clicked: Box<dyn Fn()>) -> Option<OutsideClicks> {
    use windows::Win32::UI::WindowsAndMessaging::{
        MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
    };
    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let down = matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        );
        if code >= 0 && down {
            // SAFETY: for `WH_MOUSE_LL`, `lparam` points at the event.
            let point = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) }.pt;
            OUTSIDE.with(|o| {
                if let Some((hwnd, clicked)) = &*o.borrow() {
                    let mut r = RECT::default();
                    // SAFETY: `r` is a valid out-pointer; a window gone reads as "outside".
                    let known = unsafe { GetWindowRect(*hwnd, &mut r) }.is_ok();
                    let inside = known
                        && point.x >= r.left
                        && point.x < r.right
                        && point.y >= r.top
                        && point.y < r.bottom;
                    if !inside {
                        clicked();
                    }
                }
            });
        }
        // SAFETY: passes the event on, as every hook must.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
    OUTSIDE.with(|o| *o.borrow_mut() = Some((hwnd_of(id), clicked)));
    // SAFETY: a low-level hook runs on this thread's message loop; its procedure lives as long
    // as the program.
    match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook), None, 0) } {
        Ok(h) => Some(OutsideClicks(h)),
        Err(_) => {
            OUTSIDE.with(|o| o.borrow_mut().take());
            None
        }
    }
}

/// Calls `changed` on a thread of its own each time another window comes to the foreground,
/// for as long as the program runs. `false` when Windows refused the hook.
pub fn on_foreground_change(changed: impl Fn() + Send + 'static) -> bool {
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, MSG, WINEVENT_OUTOFCONTEXT,
        WINEVENT_SKIPOWNPROCESS,
    };
    thread_local! {
        static CHANGED: std::cell::RefCell<Option<Box<dyn Fn()>>> =
            const { std::cell::RefCell::new(None) };
    }
    unsafe extern "system" fn event(
        _: HWINEVENTHOOK,
        _: u32,
        _: HWND,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
    ) {
        CHANGED.with(|c| {
            if let Some(changed) = &*c.borrow() {
                changed();
            }
        });
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("foreground".into())
        .spawn(move || {
            CHANGED.with(|c| *c.borrow_mut() = Some(Box::new(changed)));
            // SAFETY: an out-of-context hook calls `event` from this thread's message loop
            // below, which runs until the program ends.
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            let _ = tx.send(!hook.is_invalid());
            if hook.is_invalid() {
                return;
            }
            let mut msg = MSG::default();
            // SAFETY: a plain message loop on this thread.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                // SAFETY: `msg` was filled by `GetMessageW`.
                unsafe { DispatchMessageW(&msg) };
            }
        })
        .is_ok();
    spawned && rx.recv().unwrap_or(false)
}

/// Calls `ended` on a thread of its own when process `pid` ends (at once if it already has).
/// `false` when no thread could be started.
pub fn on_process_exit(pid: u32, ended: impl FnOnce() + Send + 'static) -> bool {
    use windows::Win32::System::Threading::{INFINITE, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: the handle is waited on, then closed, by the thread that receives it.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.ok();
    let process = process.map(|h| h.0 as usize);
    std::thread::Builder::new()
        .name("process-exit".into())
        .spawn(move || {
            if let Some(h) = process {
                let h = HANDLE(h as *mut c_void);
                // SAFETY: `h` is a live process handle owned by this thread.
                unsafe {
                    WaitForSingleObject(h, INFINITE);
                    let _ = CloseHandle(h);
                }
            }
            ended();
        })
        .is_ok()
}

/// The accent colour the user chose in Windows (`[r, g, b]`).
pub fn system_accent() -> Option<[u8; 3]> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut data = 0_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `data` and `size` are valid for the call; a missing value leaves `data` at 0.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\DWM"),
            w!("AccentColor"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&raw mut size),
        )
    };
    // 0xAABBGGRR
    (status.is_ok() && data != 0).then_some([data as u8, (data >> 8) as u8, (data >> 16) as u8])
}

/// Dresses a normal window the Windows 11 way: the title bar follows the theme and takes the
/// colour of the window, so the two look like one. Older Windows ignore the parts they lack.
pub fn style_window(id: WindowId, dark: bool, caption: [u8; 3]) -> Result<()> {
    // DWMWA_USE_IMMERSIVE_DARK_MODE = 20, DWMWA_BORDER_COLOR = 34, DWMWA_CAPTION_COLOR = 35,
    // DWMWA_TEXT_COLOR = 36 (the colours are COLORREFs: 0x00BBGGRR).
    let hwnd = hwnd_of(id);
    let set = |attribute: i32, value: u32| {
        // SAFETY: `value` is a live u32 of the size the attribute wants.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWINDOWATTRIBUTE(attribute),
                (&raw const value).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        }
    };
    let colorref = |[r, g, b]: [u8; 3]| u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16;
    let text = if dark { [255, 255, 255] } else { [26, 26, 26] };
    let result = set(20, u32::from(dark));
    let _ = set(35, colorref(caption));
    let _ = set(34, colorref(caption));
    let _ = set(36, colorref(text));
    result.map_err(|e| PlatformError::Os(e.to_string()))
}

/// The user's locale tag (`fr-FR`), for the `auto` language.
pub fn user_locale() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    // LOCALE_NAME_MAX_LENGTH
    let mut buf = [0u16; 85];
    // SAFETY: the buffer is valid for writes and its length is passed to the call.
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    let len = usize::try_from(len).ok()?.checked_sub(1)?; // drop the NUL
    String::from_utf16(buf.get(..len)?).ok()
}

/// Holds the name for as long as it lives; a second holder of the same name gets `None`.
pub struct InstanceGuard(HANDLE);

// SAFETY: a mutex handle can be closed from any thread.
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `single_instance` and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Takes the per-session instance lock called `name`: `Some` for the first caller.
pub fn single_instance(name: &str) -> Option<InstanceGuard> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    let wide: Vec<u16> = format!("Local\\{name}\0").encode_utf16().collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let handle = unsafe { CreateMutexW(None, false, windows::core::PCWSTR(wide.as_ptr())) }.ok()?;
    // SAFETY: reads the thread's last error, set by the call above.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: closing the handle we just got.
        let _ = unsafe { CloseHandle(handle) };
        return None;
    }
    Some(InstanceGuard(handle))
}

/// The application identity of every Vixeeny process: their windows group under one taskbar
/// button, with the name and icon of the Start menu shortcut (which carries the same id).
pub const APP_ID: &str = "Xantoom.Vixeeny";

pub fn set_app_id() -> Result<()> {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let wide: Vec<u16> = APP_ID.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    unsafe { SetCurrentProcessExplicitAppUserModelID(windows::core::PCWSTR(wide.as_ptr())) }
        .map_err(|e| os_err("SetCurrentProcessExplicitAppUserModelID", e))
}

/// Marks a window of this process with `tag`, so that another process can find it again with
/// [`focus_tagged_window`] (every window of Vixeeny has the same title).
pub fn tag_window(id: WindowId, tag: &str) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::SetPropW;
    let wide: Vec<u16> = tag.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; the property holds no pointer
    // (a non-null marker value), so nothing must be freed when the window goes away.
    unsafe {
        SetPropW(
            hwnd_of(id),
            windows::core::PCWSTR(wide.as_ptr()),
            Some(HANDLE(std::ptr::dangling_mut::<c_void>())),
        )
    }
    .map_err(|e| os_err("SetPropW", e))
}

/// Lets another process (the app) take the foreground. Called by the daemon when a shortcut
/// hands an action over: the shortcut gave the daemon the right to, and the editor needs the
/// keyboard as soon as it appears.
pub fn allow_foreground_handoff() {
    use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
    // SAFETY: plain call; fails harmlessly when this process may not give the foreground away.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

/// Brings the top-level window marked with `tag` (see [`tag_window`]) to the front. `true` when
/// there was one.
pub fn focus_tagged_window(tag: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetPropW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    let wide: Vec<u16> = tag.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the enumeration.
    if unsafe { EnumWindows(Some(collect_window), LPARAM(&raw mut handles as isize)) }.is_err() {
        return false;
    }
    for handle in handles {
        // SAFETY: `wide` is NUL-terminated; the window may belong to another process, which
        // these calls are allowed to address.
        unsafe {
            if GetPropW(handle, windows::core::PCWSTR(wide.as_ptr())).is_invalid() {
                continue;
            }
            if IsIconic(handle).as_bool() {
                let _ = ShowWindow(handle, SW_RESTORE);
            }
            return SetForegroundWindow(handle).as_bool();
        }
    }
    false
}

/// Opens a file, folder or address with the program Windows associates with it.
pub fn open_path(path: &str) -> Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(PlatformError::Os(format!("cannot open {path}")))
    }
}

/// Opens the folder of `path` in the Explorer with the file selected.
pub fn reveal(path: &str) -> Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};
    let args: Vec<u16> = format!("/select,\"{path}\"")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: every string is NUL-terminated and outlives the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            w!("explorer.exe"),
            PCWSTR(args.as_ptr()),
            None,
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(PlatformError::Os(format!("cannot show {path}")))
    }
}

/// Looks up the DXGI output of `monitor`: `(is HDR, peak nits, GDI device name)`.
fn dxgi_output(monitor: MonitorId) -> Option<(bool, f32, [u16; 32])> {
    // SAFETY: plain DXGI enumeration; every interface is reference counted by the bindings.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            adapter_index += 1;
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                output_index += 1;
                let Ok(output6) = output.cast::<IDXGIOutput6>() else {
                    continue;
                };
                let Ok(desc) = output6.GetDesc1() else {
                    continue;
                };
                if desc.Monitor.0 as usize as u64 == monitor.0 {
                    let hdr = desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
                    return Some((hdr, desc.MaxLuminance, desc.DeviceName));
                }
            }
        }
        None
    }
}

/// SDR white level of the display whose GDI name is `gdi_name`, in nits.
fn sdr_white_nits(gdi_name: &[u16; 32]) -> Option<f32> {
    // SAFETY: the buffers are sized by `GetDisplayConfigBufferSizes`; the request structs have
    // their header size and type set before each call.
    unsafe {
        let (mut paths_len, mut modes_len) = (0u32, 0u32);
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, &mut modes_len)
            .ok()
            .ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); paths_len as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); modes_len as usize];
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths_len,
            paths.as_mut_ptr(),
            &mut modes_len,
            modes.as_mut_ptr(),
            None,
        )
        .ok()
        .ok()?;
        for path in paths.iter().take(paths_len as usize) {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&raw mut source.header) != 0
                || source.viewGdiDeviceName != *gdi_name
            {
                continue;
            }
            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&raw mut white.header) == 0 {
                // 1000 means 80 nits.
                return Some(white.SDRWhiteLevel as f32 / 1000.0 * 80.0);
            }
        }
        None
    }
}

/// HDR parameters of `monitor`, or `None` when it shows SDR (or they cannot be read, in which
/// case capturing as SDR is the safe choice).
pub fn hdr_info(monitor: MonitorId) -> Option<HdrInfo> {
    let (hdr, peak, name) = dxgi_output(monitor)?;
    if !hdr {
        return None;
    }
    Some(HdrInfo {
        sdr_white_nits: sdr_white_nits(&name).unwrap_or(80.0),
        peak_nits: if peak > 0.0 { peak } else { 1000.0 },
    })
}

/// `ProductName` and `FileDescription` from the executable's version resource. Missing or
/// unreadable resources give empty fields, never an error.
pub fn exe_metadata(path: &str) -> crate::ExeMetadata {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = 0u32;
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), Some(&raw mut handle)) };
    if size == 0 {
        return crate::ExeMetadata::default();
    }
    let mut block = vec![0u8; size as usize];
    // SAFETY: `block` is `size` bytes long, as `GetFileVersionInfoW` requires.
    if unsafe { GetFileVersionInfoW(PCWSTR(wide.as_ptr()), None, size, block.as_mut_ptr().cast()) }
        .is_err()
    {
        return crate::ExeMetadata::default();
    }

    let query = |sub: &str| -> Option<(*const c_void, u32)> {
        let sub: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: `block` holds a valid version resource; the out-pointers are valid and the
        // returned pointer points into `block`, which outlives every use below.
        let ok = unsafe {
            VerQueryValueW(
                block.as_ptr().cast(),
                PCWSTR(sub.as_ptr()),
                &raw mut ptr,
                &raw mut len,
            )
        };
        (ok.as_bool() && !ptr.is_null() && len > 0).then_some((ptr.cast_const(), len))
    };
    // Language/code page pairs; English (US) Unicode as a last resort.
    let mut langs: Vec<String> = Vec::new();
    if let Some((ptr, len)) = query("\\VarFileInfo\\Translation") {
        // SAFETY: the translation table is `len` bytes of (u16 language, u16 code page) pairs.
        let words = unsafe { std::slice::from_raw_parts(ptr.cast::<u16>(), len as usize / 2) };
        langs.extend(
            words
                .as_chunks::<2>()
                .0
                .iter()
                .map(|w| format!("{:04x}{:04x}", w[0], w[1])),
        );
    }
    langs.push("040904b0".to_owned());

    let string = |name: &str| -> Option<String> {
        langs.iter().find_map(|lang| {
            let (ptr, len) = query(&format!("\\StringFileInfo\\{lang}\\{name}"))?;
            // SAFETY: a string value is `len` UTF-16 units including the terminating NUL.
            let units = unsafe { std::slice::from_raw_parts(ptr.cast::<u16>(), len as usize) };
            let text = String::from_utf16_lossy(units);
            let text = text.trim_end_matches('\0').trim();
            (!text.is_empty()).then(|| text.to_owned())
        })
    };
    crate::ExeMetadata {
        product_name: string("ProductName"),
        file_description: string("FileDescription"),
    }
}

/// The graphics adapters, in DXGI order.
pub fn gpu_adapters() -> Result<Vec<crate::GpuInfo>> {
    use windows::core::Interface;
    // SAFETY: plain DXGI enumeration; every interface is released when dropped.
    unsafe {
        let factory: IDXGIFactory1 =
            CreateDXGIFactory1().map_err(|e| PlatformError::Os(format!("DXGI factory: {e}")))?;
        let mut out = Vec::new();
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            index += 1;
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            let len = desc
                .Description
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(desc.Description.len());
            let driver = adapter
                .CheckInterfaceSupport(&IDXGIDevice::IID)
                .map(crate::format_driver_version)
                .unwrap_or_default();
            out.push(crate::GpuInfo {
                name: String::from_utf16_lossy(&desc.Description[..len]),
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                driver_version: driver,
                software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
            });
        }
        Ok(out)
    }
}

/// Lets a GUI-subsystem process print to the terminal that started it (command-line modes such
/// as `--probe-report`). A no-op when it was not started from a console.
pub fn attach_console() {
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows::core::w;
    // SAFETY: console attachment and handle replacement have no memory-safety preconditions;
    // the handle opened here is intentionally kept for the life of the process.
    unsafe {
        // Output redirected to a file or a pipe (`> report.txt`) stays there.
        let missing = |which: STD_HANDLE| GetStdHandle(which).is_ok_and(|h| h.is_invalid());
        let (out_missing, err_missing) = (missing(STD_OUTPUT_HANDLE), missing(STD_ERROR_HANDLE));
        if !out_missing && !err_missing {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        if let Ok(out) = CreateFileW(
            w!("CONOUT$"),
            GENERIC_WRITE.0,
            FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        ) {
            if out_missing {
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, out);
            }
            if err_missing {
                let _ = SetStdHandle(STD_ERROR_HANDLE, out);
            }
        }
    }
}

/// Nanoseconds on the performance counter: the clock of WGC's `SystemRelativeTime`.
pub fn monotonic_ns() -> i64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut counter, mut frequency) = (0i64, 0i64);
    // SAFETY: both out-pointers are valid; these calls cannot fail on Windows XP and later.
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    let frequency = i128::from(frequency.max(1));
    (i128::from(counter) * 1_000_000_000 / frequency) as i64
}

/// The icon of an executable, `size` pixels square, as RGBA rows (straight alpha). `None` when
/// the file has no icon or cannot be read.
pub fn exe_icon(path: &str, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
        GetDIBits, GetObjectW, ReleaseDC,
    };
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut icon = HICON::default();
    // SAFETY: `wide` is a NUL-terminated path; the icon handle is destroyed below.
    unsafe {
        SHDefExtractIconW(
            windows::core::PCWSTR(wide.as_ptr()),
            0,
            0,
            Some(&raw mut icon),
            None,
            size,
        )
    }
    .ok()
    .ok()?;
    if icon.is_invalid() {
        return None;
    }
    let mut info = ICONINFO::default();
    // SAFETY: a live icon; its bitmaps are deleted below.
    let got = unsafe { GetIconInfo(icon, &raw mut info) };
    // SAFETY: the icon is ours and no longer used after this.
    let _ = unsafe { DestroyIcon(icon) };
    got.ok()?;
    let color = info.hbmColor;
    let pixels = (|| {
        if color.is_invalid() {
            return None; // a monochrome icon: not worth showing
        }
        let mut bitmap = BITMAP::default();
        // SAFETY: reads the size of a live bitmap into a buffer of the right size.
        let n = unsafe {
            GetObjectW(
                color.into(),
                size_of::<BITMAP>() as i32,
                Some((&raw mut bitmap).cast()),
            )
        };
        if n == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
            return None;
        }
        let (w, h) = (bitmap.bmWidth as u32, bitmap.bmHeight as u32);
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        // SAFETY: the screen DC is released; the buffer holds `h` rows of `w` 32-bit pixels.
        let rows = unsafe {
            let dc = GetDC(None);
            let rows = GetDIBits(
                dc,
                color,
                0,
                h,
                Some(bgra.as_mut_ptr().cast()),
                &raw mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, dc);
            rows
        };
        if rows != h as i32 {
            return None;
        }
        // Old icons have no alpha: their mask says what is transparent; shown opaque instead.
        let opaque = bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
        for p in bgra.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
            if opaque {
                p[3] = 255;
            }
        }
        Some((w, h, bgra))
    })();
    // SAFETY: the bitmaps GetIconInfo created are ours to delete.
    unsafe {
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_foreground_can_be_watched() {
        assert!(super::on_foreground_change(|| {}));
    }

    #[test]
    fn the_end_of_a_process_is_reported() {
        use std::time::Duration;
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 2 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(super::on_process_exit(child.id(), move || tx
            .send(())
            .unwrap()));
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "still running"
        );
        child.wait().unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // A process already gone: at once.
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(super::on_process_exit(child.id(), move || tx
            .send(())
            .unwrap()));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_program_icon_is_read_with_its_transparency() {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let (w, h, rgba) = super::exe_icon(&format!(r"{windir}\explorer.exe"), 48)
            .expect("explorer.exe has an icon");
        assert_eq!((w, h), (48, 48));
        assert_eq!(rgba.len(), 48 * 48 * 4);
        // Drawn on a transparent ground: some pixels clear, some solid.
        assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 0));
        assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        assert!(super::exe_icon(r"C:\nowhere\nothing.exe", 48).is_none());
    }
}
