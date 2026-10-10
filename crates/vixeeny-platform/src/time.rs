// SPDX-License-Identifier: GPL-3.0-or-later
//! Wall-clock time in the user's time zone, for file names (`{date}`, `{time}`, `{ms}`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millisecond: u16,
}

impl LocalTime {
    /// `YYYY-MM-DD`
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `HH-MM-SS` (no colons: valid in file names).
    pub fn time(&self) -> String {
        format!("{:02}-{:02}-{:02}", self.hour, self.minute, self.second)
    }

    /// Civil time of a Unix timestamp, UTC (days-from-civil algorithm by H. Hinnant).
    pub fn from_unix_utc(secs: u64, millisecond: u16) -> Self {
        let days = (secs / 86_400) as i64;
        let rem = secs % 86_400;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
        Self {
            year,
            month,
            day,
            hour: (rem / 3600) as u8,
            minute: (rem % 3600 / 60) as u8,
            second: (rem % 60) as u8,
            millisecond,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_conversion() {
        let t = LocalTime::from_unix_utc(0, 0);
        assert_eq!(
            (t.date(), t.time()),
            ("1970-01-01".into(), "00-00-00".into())
        );
        // 2026-10-01 17:12:00 UTC
        let t = LocalTime::from_unix_utc(1_790_874_720, 5);
        assert_eq!(
            (t.date(), t.time(), t.millisecond),
            ("2026-10-01".into(), "17-12-00".into(), 5)
        );
        // Leap day.
        assert_eq!(
            LocalTime::from_unix_utc(1_709_164_800, 0).date(),
            "2024-02-29"
        );
    }
}
