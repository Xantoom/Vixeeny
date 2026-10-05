// SPDX-License-Identifier: GPL-3.0-or-later
//! File names and per-application folders (plan 5.8): application name resolution, name
//! sanitising, filename templates. Pure functions, no I/O except the `exists` callback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Names used when nothing better is known.
pub const DEFAULT_APP_NAME: &str = "Vixeeny";
/// `{app}` of a desktop capture when no full-screen application has the focus.
pub const DESKTOP_APP_NAME: &str = "Desktop";
/// Longest file or folder name, in characters.
pub const MAX_NAME_CHARS: usize = 100;

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turns arbitrary text into a name every file system accepts: forbidden characters removed,
/// control characters dropped, no trailing space or dot, Windows reserved names prefixed,
/// at most [`MAX_NAME_CHARS`] characters. Never returns an empty string.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let mut cleaned: String = cleaned.trim().chars().take(MAX_NAME_CHARS).collect();
    // Truncation can expose a trailing space or dot again.
    while cleaned.ends_with([' ', '.']) {
        cleaned.pop();
    }
    if cleaned.is_empty() {
        return DEFAULT_APP_NAME.to_owned();
    }
    let stem = cleaned
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        cleaned.insert(0, '_');
    }
    cleaned
}

/// Whether an executable name says nothing about the product (Unreal Engine shipping builds,
/// platform suffixes, generic words).
pub fn is_generic_exe_name(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    lower.contains("shipping")
        || lower.contains("win64")
        || lower.contains("win32")
        || lower.contains("x64")
        || matches!(
            lower.as_str(),
            "game"
                | "client"
                | "main"
                | "app"
                | "application"
                | "launcher"
                | "start"
                | "run"
                | "play"
        )
}

/// `C:\Games\Foo\Client-Win64-Shipping.exe` → `Client-Win64-Shipping`.
pub fn exe_stem(exe_path: &str) -> &str {
    let file = exe_path.rsplit(['\\', '/']).next().unwrap_or(exe_path);
    file.rsplit_once('.').map_or(file, |(stem, ext)| {
        if ext.eq_ignore_ascii_case("exe") {
            stem
        } else {
            file
        }
    })
}

/// What is known about the application that owns a window.
#[derive(Debug, Clone, Copy, Default)]
pub struct AppInfo<'a> {
    pub exe_path: Option<&'a str>,
    /// Version-resource `ProductName`.
    pub product_name: Option<&'a str>,
    /// Version-resource `FileDescription`.
    pub file_description: Option<&'a str>,
    pub window_title: Option<&'a str>,
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// Resolves the display name in the order of plan 5.8: user table, `ProductName`,
/// `FileDescription`, window title, executable name (unless generic). `None` means unknown:
/// callers use [`DEFAULT_APP_NAME`]. The result is already sanitised.
pub fn resolve_app_name(info: &AppInfo<'_>, user_map: &BTreeMap<String, String>) -> Option<String> {
    let stem = info.exe_path.map(exe_stem);
    if let Some(stem) = stem {
        let hit = user_map.iter().find(|(k, _)| {
            let key = exe_stem(k);
            key.eq_ignore_ascii_case(stem)
        });
        if let Some((_, name)) = hit.filter(|(_, n)| !n.trim().is_empty()) {
            return Some(sanitize(name));
        }
    }
    let candidates = [
        non_empty(info.product_name),
        non_empty(info.file_description),
        non_empty(info.window_title),
        stem.filter(|s| !s.is_empty() && !is_generic_exe_name(s)),
    ];
    candidates.into_iter().flatten().next().map(sanitize)
}

/// Values for the template variables.
#[derive(Debug, Clone, Default)]
pub struct Vars {
    pub app: String,
    pub title: String,
    pub date: String,
    pub time: String,
    pub millis: u16,
    pub width: u32,
    pub height: u32,
    pub monitor: String,
}

/// Expands `{app}`, `{title}`, `{date}`, `{time}`, `{ms}`, `{width}`, `{height}`, `{monitor}`
/// and `{counter}` (replaced by `counter`). Unknown variables expand to nothing. The result is
/// sanitised.
pub fn expand_template(template: &str, vars: &Vars, counter: u32) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        match &after[..end] {
            "app" => out.push_str(&vars.app),
            "title" => out.push_str(&vars.title),
            "date" => out.push_str(&vars.date),
            "time" => out.push_str(&vars.time),
            "ms" => out.push_str(&format!("{:03}", vars.millis)),
            "counter" => out.push_str(&counter.to_string()),
            "width" => out.push_str(&vars.width.to_string()),
            "height" => out.push_str(&vars.height.to_string()),
            "monitor" => out.push_str(&vars.monitor),
            _ => {}
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    sanitize(&out)
}

/// Where a capture is written: `<dir>[/<app>]/<name>.<ext>`. A free file name is found with
/// `exists`: with `{counter}` in the template the counter is incremented, otherwise `_2`, `_3`…
/// is appended.
pub fn output_path(
    dir: &Path,
    per_app_subfolder: bool,
    template: &str,
    vars: &Vars,
    ext: &str,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let dir = if per_app_subfolder {
        dir.join(sanitize(&vars.app))
    } else {
        dir.to_path_buf()
    };
    let has_counter = template.contains("{counter}");
    for n in 1u32.. {
        let stem = expand_template(template, vars, n);
        let name = if has_counter || n == 1 {
            stem
        } else {
            format!("{stem}_{n}")
        };
        let path = dir.join(format!("{name}.{ext}"));
        if !exists(&path) {
            return path;
        }
    }
    unreachable!("the counter loop only ends by returning")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn sanitize_removes_forbidden_characters() {
        assert_eq!(sanitize(r#"a<b>c:d"e/f\g|h?i*j"#), "abcdefghij");
        assert_eq!(sanitize("  name. . "), "name");
        assert_eq!(sanitize("tab\there\nnewline"), "tabherenewline");
        assert_eq!(sanitize("Wuthering Waves"), "Wuthering Waves");
        assert_eq!(sanitize("日本語のゲーム"), "日本語のゲーム");
    }

    #[test]
    fn sanitize_handles_reserved_and_empty_names() {
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul.txt"), "_nul.txt");
        assert_eq!(sanitize("com1"), "_com1");
        assert_eq!(sanitize("CONSOLE"), "CONSOLE");
        assert_eq!(sanitize(""), DEFAULT_APP_NAME);
        assert_eq!(sanitize("???"), DEFAULT_APP_NAME);
    }

    #[test]
    fn sanitize_limits_the_length_by_characters() {
        let long = "é".repeat(300);
        assert_eq!(sanitize(&long).chars().count(), MAX_NAME_CHARS);
        let dots = format!("{}...", "a".repeat(99));
        assert_eq!(sanitize(&dots), "a".repeat(99));
    }

    #[test]
    fn exe_stems() {
        assert_eq!(
            exe_stem(r"C:\Games\Foo\Client-Win64-Shipping.exe"),
            "Client-Win64-Shipping"
        );
        assert_eq!(exe_stem("/usr/bin/firefox"), "firefox");
        assert_eq!(exe_stem("tool.EXE"), "tool");
        assert_eq!(exe_stem("archive.tar.gz"), "archive.tar.gz");
    }

    #[test]
    fn generic_names() {
        assert!(is_generic_exe_name("Client-Win64-Shipping"));
        assert!(is_generic_exe_name("Game"));
        assert!(is_generic_exe_name("MyGame-x64"));
        assert!(!is_generic_exe_name("Photoshop"));
        assert!(!is_generic_exe_name("WutheringWaves"));
    }

    #[test]
    fn resolution_order_user_table_first() {
        let info = AppInfo {
            exe_path: Some(r"C:\G\Client-Win64-Shipping.exe"),
            product_name: Some("Product"),
            file_description: Some("Description"),
            window_title: Some("Title"),
        };
        let table = map(&[("client-win64-shipping.exe", "Wuthering Waves")]);
        assert_eq!(
            resolve_app_name(&info, &table).as_deref(),
            Some("Wuthering Waves")
        );
        // Without a table entry: ProductName, then FileDescription, then title.
        let none = BTreeMap::new();
        assert_eq!(resolve_app_name(&info, &none).as_deref(), Some("Product"));
        let info = AppInfo {
            product_name: None,
            ..info
        };
        assert_eq!(
            resolve_app_name(&info, &none).as_deref(),
            Some("Description")
        );
        let info = AppInfo {
            file_description: Some("  "),
            ..info
        };
        assert_eq!(resolve_app_name(&info, &none).as_deref(), Some("Title"));
    }

    #[test]
    fn generic_exe_falls_back_to_unknown_but_title_wins() {
        let generic = AppInfo {
            exe_path: Some("Client-Win64-Shipping.exe"),
            ..AppInfo::default()
        };
        assert_eq!(resolve_app_name(&generic, &BTreeMap::new()), None);
        let with_title = AppInfo {
            window_title: Some("Wuthering Waves"),
            ..generic
        };
        assert_eq!(
            resolve_app_name(&with_title, &BTreeMap::new()).as_deref(),
            Some("Wuthering Waves")
        );
        let plain = AppInfo {
            exe_path: Some("C:/x/Blender.exe"),
            ..AppInfo::default()
        };
        assert_eq!(
            resolve_app_name(&plain, &BTreeMap::new()).as_deref(),
            Some("Blender")
        );
    }

    #[test]
    fn user_table_ignores_blank_names_and_extension_in_key() {
        let info = AppInfo {
            exe_path: Some("a/Foo.exe"),
            product_name: Some("P"),
            ..AppInfo::default()
        };
        assert_eq!(
            resolve_app_name(&info, &map(&[("Foo", "Bar")])).as_deref(),
            Some("Bar")
        );
        assert_eq!(
            resolve_app_name(&info, &map(&[("foo.exe", "  ")])).as_deref(),
            Some("P")
        );
    }

    fn vars() -> Vars {
        Vars {
            app: "Wuthering Waves".into(),
            title: "Title: x".into(),
            date: "2026-10-01".into(),
            time: "17-12-00".into(),
            millis: 7,
            width: 3840,
            height: 2160,
            monitor: "1".into(),
        }
    }

    #[test]
    fn default_template() {
        assert_eq!(
            expand_template("{app}_{date}_{time}", &vars(), 1),
            "Wuthering Waves_2026-10-01_17-12-00"
        );
    }

    #[test]
    fn all_variables() {
        let t = "{title}-{ms}-{width}x{height}-m{monitor}-{counter}-{nope}";
        assert_eq!(
            expand_template(t, &vars(), 5),
            "Title x-007-3840x2160-m1-5-"
        );
        assert_eq!(
            expand_template("a{unterminated", &vars(), 1),
            "a{unterminated"
        );
    }

    #[test]
    fn template_output_is_sanitised() {
        let mut v = vars();
        v.app = "A/B".into();
        assert_eq!(expand_template("{app}?", &v, 1), "AB");
    }

    #[test]
    fn output_path_with_subfolder_and_collisions() {
        let dir = Path::new("pics");
        let taken: Vec<PathBuf> = vec![
            Path::new("pics")
                .join("Wuthering Waves")
                .join("Wuthering Waves_2026-10-01_17-12-00.png"),
        ];
        let exists = |p: &Path| taken.iter().any(|t| t == p);
        let p = output_path(dir, true, "{app}_{date}_{time}", &vars(), "png", exists);
        assert_eq!(
            p,
            Path::new("pics")
                .join("Wuthering Waves")
                .join("Wuthering Waves_2026-10-01_17-12-00_2.png")
        );
        let flat = output_path(dir, false, "{app}_{date}_{time}", &vars(), "png", |_| false);
        assert_eq!(
            flat,
            Path::new("pics").join("Wuthering Waves_2026-10-01_17-12-00.png")
        );
    }

    #[test]
    fn counter_template_increments_the_counter() {
        let taken = |p: &Path| p.ends_with("shot_1.png") || p.ends_with("shot_2.png");
        let p = output_path(
            Path::new("d"),
            false,
            "shot_{counter}",
            &vars(),
            "png",
            taken,
        );
        assert_eq!(p, Path::new("d").join("shot_3.png"));
    }
}
