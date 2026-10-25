// SPDX-License-Identifier: GPL-3.0-or-later
//! The local time and the version of Windows.

use vixeeny_common::time::LocalTime;

/// The time now, with the offset of the time zone.
pub fn local_time() -> LocalTime {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: no arguments; returns the current local time by value.
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear,
        month: t.wMonth as u8,
        day: t.wDay as u8,
        hour: t.wHour as u8,
        minute: t.wMinute as u8,
        second: t.wSecond as u8,
        millisecond: t.wMilliseconds,
        offset_minutes: utc_offset_minutes(),
    }
}

/// Minutes to add to UTC for the local time (`+120` in Paris in summer).
pub fn utc_offset_minutes() -> i16 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    /// What `GetTimeZoneInformation` returns while daylight saving time is on.
    const DAYLIGHT: u32 = 2;
    let mut tz = TIME_ZONE_INFORMATION::default();
    // SAFETY: `tz` is a valid out-pointer.
    let state = unsafe { GetTimeZoneInformation(&raw mut tz) };
    let bias = tz.Bias
        + if state == DAYLIGHT {
            tz.DaylightBias
        } else {
            tz.StandardBias
        };
    (-bias) as i16
}

/// "Windows 11 24H2 (build 26100.4061)", from the registry (no process started).
pub fn os_version() -> String {
    use windows::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::{PCWSTR, w};
    const KEY: PCWSTR = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let text = |name: PCWSTR| -> Option<String> {
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: `buf` and `size` describe a valid buffer for the call.
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                KEY,
                name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&raw mut size),
            )
        }
        .ok()
        .ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    };
    let number = |name: PCWSTR| -> Option<u32> {
        let mut value = 0u32;
        let mut size = 4u32;
        // SAFETY: `value` and `size` describe a valid buffer for the call.
        unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                KEY,
                name,
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut value).cast()),
                Some(&raw mut size),
            )
        }
        .ok()
        .ok()?;
        Some(value)
    };
    let build: u32 = text(w!("CurrentBuildNumber"))
        .and_then(|b| b.parse().ok())
        .unwrap_or(0);
    // Windows 11 still calls itself Windows 10 in `ProductName`: the build tells them apart.
    let name = if build >= 22_000 {
        "Windows 11"
    } else {
        "Windows 10"
    };
    let release = text(w!("DisplayVersion")).map_or_else(String::new, |v| format!(" {v}"));
    let patch = number(w!("UBR")).map_or_else(String::new, |u| format!(".{u}"));
    format!("{name}{release} (build {build}{patch})")
}
