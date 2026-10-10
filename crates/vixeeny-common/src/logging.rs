// SPDX-License-Identifier: GPL-3.0-or-later
//! Log files (plan 0.3): `<log dir>/<name>.<local date>.log`, one line per event in local time
//! with its UTC offset, so that a user's "around 11 pm" matches. Light even after years of daily
//! use: a week of files per program, at most [`DAY_LIMIT`] bytes a day, and a line repeated in a
//! row is written once with a count. `RUST_LOG` overrides the `info` default. The writer is
//! synchronous: no helper thread, no wake-ups while idle.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;

/// Files kept per program (one a day).
pub const KEEP_DAYS: usize = 7;
/// Bytes written a day per program at most; past it, one line says so and the rest is dropped.
pub const DAY_LIMIT: u64 = 1 << 20;

/// Local wall-clock time and its offset from UTC (the platform layer reads them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
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

impl Stamp {
    /// `YYYY-MM-DD`
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl std::fmt::Display for Stamp {
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

/// Reads the local time.
pub type Clock = fn() -> Stamp;

/// Length of a formatted [`Stamp`]: what precedes the message on each line.
const STAMP_LEN: usize = "2026-10-10 16:32:05.123 +02:00".len();

struct LocalTimer(Clock);

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        write!(w, "{}", (self.0)())
    }
}

/// The day's file of one program. Each `write` is one event (the formatter writes a whole line
/// at once).
pub struct DailyFile {
    dir: PathBuf,
    name: String,
    clock: Clock,
    date: String,
    file: Option<File>,
    written: u64,
    full: bool,
    /// The previous line without its time, and how many times it came again since.
    last: Vec<u8>,
    repeats: u32,
    last_repeat: Option<Stamp>,
}

impl DailyFile {
    pub fn new(dir: &Path, name: &str, clock: Clock) -> Self {
        Self {
            dir: dir.to_path_buf(),
            name: name.to_owned(),
            clock,
            date: String::new(),
            file: None,
            written: 0,
            full: false,
            last: Vec::new(),
            repeats: 0,
            last_repeat: None,
        }
    }

    fn open(&mut self, date: String) {
        let path = self.dir.join(format!("{}.{date}.log", self.name));
        self.file = OpenOptions::new().create(true).append(true).open(path).ok();
        self.written = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        self.full = self.written >= DAY_LIMIT;
        self.date = date;
        self.prune();
    }

    /// Removes the files of this program beyond the newest [`KEEP_DAYS`].
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let prefix = format!("{}.", self.name);
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&prefix) && n.ends_with(".log"))
            .collect();
        // The date in the name sorts them.
        files.sort();
        let excess = files.len().saturating_sub(KEEP_DAYS);
        for old in &files[..excess] {
            let _ = std::fs::remove_file(self.dir.join(old));
        }
    }

    fn emit(&mut self, line: &[u8]) {
        let Some(file) = &mut self.file else { return };
        if self.full {
            return;
        }
        if self.written + line.len() as u64 > DAY_LIMIT {
            self.full = true;
            let note = format!(
                "{}  WARN the log is full for today: nothing more is written until tomorrow\n",
                (self.clock)()
            );
            let _ = file.write_all(note.as_bytes());
            return;
        }
        if file.write_all(line).is_ok() {
            self.written += line.len() as u64;
        }
    }

    fn flush_repeats(&mut self) {
        if self.repeats == 0 {
            return;
        }
        let until = self.last_repeat.map_or_else(String::new, |s| {
            format!(
                ", the last at {:02}:{:02}:{:02}",
                s.hour, s.minute, s.second
            )
        });
        let note = format!(
            "{}  INFO (the line above came {} more time(s){until})\n",
            (self.clock)(),
            self.repeats
        );
        self.repeats = 0;
        self.emit(note.as_bytes());
    }
}

impl Write for DailyFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let now = (self.clock)();
        let date = now.date();
        if date != self.date {
            self.flush_repeats();
            self.last.clear();
            self.open(date);
        }
        let body = buf.get(STAMP_LEN..).unwrap_or(buf);
        if !body.is_empty() && body == self.last.as_slice() {
            self.repeats += 1;
            self.last_repeat = Some(now);
            return Ok(buf.len());
        }
        self.flush_repeats();
        self.last = body.to_vec();
        self.emit(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Logs to `<log dir>/<name>.<date>.log`, then writes the line that opens every run: version,
/// `program` (what this process does), process id and `system` (the Windows version). Failing to
/// open the log directory is not fatal: the program then runs without logs.
pub fn init(name: &str, program: &str, clock: Clock, system: &str) {
    let Some(dir) = crate::paths::log_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // Ignore the error: a subscriber may already be installed (tests).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_timer(LocalTimer(clock))
        .with_writer(Mutex::new(DailyFile::new(&dir, name, clock)))
        .try_init();
    tracing::info!(
        "Vixeeny {} started: {program}, process {}, {system}",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
    // The programs have no console (Windows subsystem): a panic would vanish without a trace.
    // Its message gives the file and line (the release builds carry no symbols for a trace).
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        tracing::error!(
            "crash in thread `{}`: {info}",
            thread.name().unwrap_or("unnamed")
        );
    }));
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// Seconds since the start of 2026-10-10, local; a test moves it.
    static NOW: AtomicU32 = AtomicU32::new(0);

    fn clock() -> Stamp {
        let t = NOW.load(Ordering::Relaxed);
        Stamp {
            year: 2026,
            month: 10,
            day: 10 + (t / 86_400) as u8,
            hour: (t % 86_400 / 3600) as u8,
            minute: (t % 3600 / 60) as u8,
            second: (t % 60) as u8,
            millisecond: 0,
            offset_minutes: 120,
        }
    }

    fn line(text: &str) -> String {
        format!("{}  INFO {text}\n", clock())
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vixeeny-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stamps_read_as_local_time_with_the_offset() {
        let mut s = clock();
        s.offset_minutes = -330;
        assert_eq!(STAMP_LEN, s.to_string().len());
        assert!(s.to_string().ends_with(" -05:30"), "{s}");
    }

    // One test moves the shared clock: the steps run in order.
    #[test]
    fn repeats_limit_and_days() {
        let dir = scratch("days");
        let mut log = DailyFile::new(&dir, "p", clock);
        let read = |date: &str| std::fs::read_to_string(dir.join(format!("p.{date}.log"))).unwrap();

        // A line that comes again and again is written once, then counted.
        NOW.store(3600, Ordering::Relaxed);
        for _ in 0..50 {
            log.write_all(line("same").as_bytes()).unwrap();
            NOW.fetch_add(1, Ordering::Relaxed);
        }
        log.write_all(line("other").as_bytes()).unwrap();
        let text = read("2026-10-10");
        assert_eq!(text.matches("same").count(), 1, "{text}");
        assert!(
            text.contains("came 49 more time(s), the last at 01:00:49"),
            "{text}"
        );
        assert!(text.ends_with("other\n"));

        // Past the daily limit, one note and nothing more.
        let big = "x".repeat(4000);
        for i in 0..400 {
            log.write_all(line(&format!("{i} {big}")).as_bytes())
                .unwrap();
        }
        let len = std::fs::metadata(dir.join("p.2026-10-10.log"))
            .unwrap()
            .len();
        assert!(len <= DAY_LIMIT + 200, "{len}");
        assert_eq!(read("2026-10-10").matches("the log is full").count(), 1);

        // A new day, a new file; only the newest files stay.
        for day in 1..=9 {
            NOW.store(day * 86_400, Ordering::Relaxed);
            log.write_all(line("a day").as_bytes()).unwrap();
        }
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), KEEP_DAYS);
        assert_eq!(names.last().unwrap(), "p.2026-10-19.log");
        let _ = std::fs::remove_dir_all(dir);
    }
}
