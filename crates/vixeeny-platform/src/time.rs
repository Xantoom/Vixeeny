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
    let bias = tz.Bias + if state == DAYLIGHT { tz.DaylightBias } else { tz.StandardBias };
    (-bias) as i16
}

/// "Windows 11 24H2 (build 26100.4061)", from the registry (no process started).
pub fn os_version() -> String {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let text = |name: &str| crate::registry::string(HKEY_LOCAL_MACHINE, KEY, name);
    let number = |name: &str| crate::registry::dword(HKEY_LOCAL_MACHINE, KEY, name);
    let build: u32 = text("CurrentBuildNumber").and_then(|b| b.parse().ok()).unwrap_or(0);
    // Windows 11 still calls itself Windows 10 in `ProductName`: the build tells them apart.
    let name = if build >= 22_000 { "Windows 11" } else { "Windows 10" };
    let release = text("DisplayVersion").map_or_else(String::new, |v| format!(" {v}"));
    let patch = number("UBR").map_or_else(String::new, |u| format!(".{u}"));
    format!("{name}{release} (build {build}{patch})")
}
