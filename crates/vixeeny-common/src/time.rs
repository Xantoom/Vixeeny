// SPDX-License-Identifier: GPL-3.0-or-later
//! Wall-clock time in the user's time zone, for file names (`{date}`, `{time}`, `{ms}`) and log
//! lines; the platform layer reads it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millisecond: u16,
    /// Minutes to add to UTC.
    pub offset_minutes: i16,
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
            offset_minutes: 0,
        }
    }
}

impl std::fmt::Display for LocalTime {
    /// `2026-10-10 16:32:05.123 +02:00`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sign = if self.offset_minutes < 0 { '-' } else { '+' };
        let offset = self.offset_minutes.unsigned_abs();
        write!(
            f,
            "{} {:02}:{:02}:{:02}.{:03} {sign}{:02}:{:02}",
            self.date(),
            self.hour,
            self.minute,
            self.second,
            self.millisecond,
            offset / 60,
            offset % 60
        )
    }
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
