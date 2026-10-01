// SPDX-License-Identifier: GPL-3.0-or-later
//! Win32 implementation: `EnumDisplayMonitors`, per-monitor DPI, DWM frame bounds.

use std::ffi::c_void;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SetWindowDisplayAffinity,
    WDA_EXCLUDEFROMCAPTURE, WS_EX_TOOLWINDOW,
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
    // SAFETY: `data` is the `&mut Vec<HWND>` passed by `top_level_windows()`.
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

/// Hides one of our own windows from every screen capture (plan 5.2 CA-IMG-2). Needs
/// Windows 10 2004 or later; the window shows normally on screen.
pub fn exclude_from_capture(id: WindowId) -> Result<()> {
    // SAFETY: plain call on a window handle owned by this process.
    unsafe { SetWindowDisplayAffinity(hwnd_of(id), WDA_EXCLUDEFROMCAPTURE) }
        .map_err(|e| os_err("SetWindowDisplayAffinity", e))
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
