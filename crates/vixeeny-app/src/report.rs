// SPDX-License-Identifier: GPL-3.0-or-later
//! "Report a problem": one text file with the system info and the logs of the last days, without
//! the user's name or folders, shown in the Explorer next to GitHub's bug form (already holding
//! the system info), so that the file only has to be dropped there.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use vixeeny_common::config::Config;

/// Days of logs in a report.
const DAYS: usize = 3;
/// Reports kept in their folder (the oldest go).
const KEEP: usize = 5;
const NEW_ISSUE: &str = "https://github.com/Xantoom/Vixeeny/issues/new?template=bug.yml";

/// Writes the report, shows it in the Explorer and opens the bug form.
pub fn create(config: &Config) -> anyhow::Result<PathBuf> {
    let logs = vixeeny_common::paths::log_dir().context("no logs folder")?;
    let probe = crate::probe::current(false).ok();
    let sysinfo = crate::sysinfo::format(&crate::sysinfo::collect(config, probe.as_ref()));
    let private = Private::of_this_user();

    let mut text = String::new();
    let _ = writeln!(text, "Vixeeny report, {}\n", crate::log_clock());
    text.push_str(&sysinfo);
    for name in newest_logs(&logs) {
        let content = std::fs::read_to_string(logs.join(&name)).unwrap_or_default();
        let _ = write!(text, "\n===== {name} =====\n{content}");
    }
    let text = private.hide(&text);

    let dir = logs
        .parent()
        .map_or_else(|| logs.clone(), |d| d.join("reports"));
    std::fs::create_dir_all(&dir).context("cannot create the reports folder")?;
    let t = vixeeny_platform::local_time();
    let path = dir.join(format!("Vixeeny-report-{}_{}.txt", t.date(), t.time()));
    std::fs::write(&path, text).context("cannot write the report")?;
    prune(&dir);

    let _ = vixeeny_platform::reveal(&path.display().to_string());
    let _ = vixeeny_platform::open_path(&issue_url(&sysinfo));
    Ok(path)
}

/// The log files of the newest [`DAYS`] days, newest day first (the daemon's, then the app's).
fn newest_logs(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("vixeeny-") && n.ends_with(".log"))
        .collect();
    pick(names)
}

/// `vixeeny-<program>.<date>.log` names → those of the newest days, newest first.
fn pick(mut names: Vec<String>) -> Vec<String> {
    let date = |n: &str| n.split('.').nth(1).unwrap_or_default().to_owned();
    let mut days: Vec<String> = names.iter().map(|n| date(n)).collect();
    days.sort();
    days.dedup();
    let newest: Vec<String> = days.into_iter().rev().take(DAYS).collect();
    names.retain(|n| newest.contains(&date(n)));
    names.sort_by(|a, b| date(b).cmp(&date(a)).then_with(|| b.cmp(a)));
    names
}

/// Removes the reports beyond the newest [`KEEP`].
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut reports: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("Vixeeny-report-") && n.ends_with(".txt"))
        .collect();
    reports.sort();
    let excess = reports.len().saturating_sub(KEEP);
    for old in &reports[..excess] {
        let _ = std::fs::remove_file(dir.join(old));
    }
}

/// GitHub's bug form with the system info filled in (the field `sysinfo` of `bug.yml`).
fn issue_url(sysinfo: &str) -> String {
    let mut url = format!("{NEW_ISSUE}&sysinfo=");
    for b in sysinfo.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            url.push(char::from(b));
        } else {
            let _ = write!(url, "%{b:02X}");
        }
    }
    url
}

/// What would tell who the user is: their folder, account and computer names.
struct Private {
    /// `(found, shown instead)`, longest first.
    words: Vec<(String, &'static str)>,
}

impl Private {
    fn of_this_user() -> Self {
        let var = |name| std::env::var(name).ok().filter(|v: &String| v.len() > 1);
        Self::new(
            var("USERPROFILE").as_deref(),
            var("USERNAME").as_deref(),
            var("COMPUTERNAME").as_deref(),
        )
    }

    fn new(profile: Option<&str>, user: Option<&str>, computer: Option<&str>) -> Self {
        let mut words = Vec::new();
        if let Some(p) = profile {
            words.push((p.to_owned(), "%USERPROFILE%"));
            words.push((p.replace('\\', "/"), "%USERPROFILE%"));
        }
        if let Some(u) = user {
            words.push((u.to_owned(), "<user>"));
        }
        if let Some(c) = computer {
            words.push((c.to_owned(), "<computer>"));
        }
        words.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
        Self { words }
    }

    /// `text` with every [`Private`] word replaced, whatever its case.
    fn hide(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (word, shown) in &self.words {
            out = replace_ignoring_case(&out, word, shown);
        }
        out
    }
}

fn replace_ignoring_case(text: &str, word: &str, by: &str) -> String {
    let lower = text.to_lowercase();
    let needle = word.to_lowercase();
    // Lower-casing can change lengths outside ASCII: then only the exact spelling is replaced.
    if lower.len() != text.len() || needle.len() != word.len() {
        return text.replace(word, by);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = 0;
    for (at, _) in lower.match_indices(&needle) {
        if at < rest {
            continue;
        }
        out.push_str(&text[rest..at]);
        out.push_str(by);
        rest = at + needle.len();
    }
    out.push_str(&text[rest..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_cannot_be_recognised() {
        let private = Private::new(
            Some(r"C:\Users\Xavier"),
            Some("Xavier"),
            Some("DESKTOP-1234"),
        );
        let text = "started C:\\Users\\Xavier\\AppData\\x.exe\n\
                    cannot write c:\\users\\xavier\\Pictures\\a.png on DESKTOP-1234\n\
                    C:/Users/Xavier/Videos and xavier";
        assert_eq!(
            private.hide(text),
            "started %USERPROFILE%\\AppData\\x.exe\n\
             cannot write %USERPROFILE%\\Pictures\\a.png on <computer>\n\
             %USERPROFILE%/Videos and <user>"
        );
    }

    #[test]
    fn the_newest_days_come_first() {
        let names = [
            "vixeeny-app.2026-10-07.log",
            "vixeeny-daemon.2026-10-07.log",
            "vixeeny-daemon.2026-10-09.log",
            "vixeeny-app.2026-10-10.log",
            "vixeeny-daemon.2026-10-10.log",
            "vixeeny-daemon.2026-10-08.log",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            pick(names),
            [
                "vixeeny-daemon.2026-10-10.log",
                "vixeeny-app.2026-10-10.log",
                "vixeeny-daemon.2026-10-09.log",
                "vixeeny-daemon.2026-10-08.log",
            ]
        );
    }

    #[test]
    fn the_system_info_goes_into_the_form() {
        let url = issue_url("a b\n<é>");
        assert_eq!(url, format!("{NEW_ISSUE}&sysinfo=a%20b%0A%3C%C3%A9%3E"));
    }
}
