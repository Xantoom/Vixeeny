// SPDX-License-Identifier: GPL-3.0-or-later
//! "Copy system info" (plan M23): the block a bug report starts with. It holds what the
//! maintainers need to reproduce a problem — version, system, display, GPUs, encoders — and
//! nothing personal: no user name, no folder, no file name.

use std::fmt::Write as _;

use vixeeny_common::config::Config;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Info {
    pub version: String,
    pub build: Vec<String>,
    pub os: String,
    pub arch: String,
    /// Desktop, session type… (Linux), empty elsewhere.
    pub session: Vec<(String, String)>,
    pub monitors: Vec<String>,
    pub gpus: Vec<String>,
    pub encoders: Vec<String>,
    pub tools: Vec<(String, bool)>,
    pub settings: Vec<(String, String)>,
}

/// The report as Markdown, ready to paste into an issue.
pub fn format(info: &Info) -> String {
    let mut out = String::from("<details><summary>Vixeeny system info</summary>\n\n```text\n");
    let _ = writeln!(
        out,
        "Vixeeny   {} ({})",
        info.version,
        info.build.join(", ")
    );
    let _ = writeln!(out, "System    {} {}", info.os, info.arch);
    for (key, value) in &info.session {
        let _ = writeln!(out, "{key:<9} {value}");
    }
    let list = |out: &mut String, title: &str, items: &[String]| {
        if items.is_empty() {
            let _ = writeln!(out, "{title:<9} none");
        }
        for (i, item) in items.iter().enumerate() {
            let _ = writeln!(out, "{:<9} {item}", if i == 0 { title } else { "" });
        }
    };
    list(&mut out, "Displays", &info.monitors);
    list(&mut out, "GPUs", &info.gpus);
    list(&mut out, "Encoders", &info.encoders);
    for (i, (tool, found)) in info.tools.iter().enumerate() {
        let _ = writeln!(
            out,
            "{:<9} {tool}: {}",
            if i == 0 { "Tools" } else { "" },
            if *found { "found" } else { "missing" }
        );
    }
    for (key, value) in &info.settings {
        let _ = writeln!(out, "{key:<9} {value}");
    }
    out.push_str("```\n\n</details>\n");
    out
}

#[cfg(any(windows, target_os = "macos"))]
fn first_line(command: &str, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(command);
    command.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

/// `PRETTY_NAME` of an os-release file.
#[cfg(any(target_os = "linux", test))]
pub fn pretty_name(os_release: &str) -> Option<String> {
    os_release.lines().find_map(|line| {
        line.strip_prefix("PRETTY_NAME=")
            .map(|v| v.trim().trim_matches('"').to_owned())
    })
}

fn os_name() -> String {
    #[cfg(target_os = "linux")]
    {
        let name = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| pretty_name(&t))
            .unwrap_or_else(|| "Linux".into());
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
        format!("{name}, kernel {}", kernel.trim())
    }
    #[cfg(target_os = "macos")]
    {
        let version = first_line("sw_vers", &["-productVersion"]).unwrap_or_default();
        format!("macOS {version}")
    }
    #[cfg(windows)]
    {
        first_line("cmd", &["/C", "ver"]).unwrap_or_else(|| "Windows".into())
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        std::env::consts::OS.to_owned()
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Collects the report of this machine. `probe` is the hardware probe (cached or fresh).
pub fn collect(config: &Config, probe: Option<&vixeeny_encode::probe::ProbeResult>) -> Info {
    let mut build = vec![
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .to_owned(),
    ];
    for (on, name) in [
        (cfg!(feature = "ffmpeg"), "ffmpeg"),
        (cfg!(feature = "native-codecs"), "native-codecs"),
        (cfg!(feature = "pipewire"), "pipewire"),
    ] {
        if on {
            build.push(name.to_owned());
        }
    }
    let mut session = Vec::new();
    let mut tools = Vec::new();
    if cfg!(target_os = "linux") {
        for (label, var) in [
            ("Desktop", "XDG_CURRENT_DESKTOP"),
            ("Session", "XDG_SESSION_TYPE"),
        ] {
            if let Ok(value) = std::env::var(var) {
                session.push((label.to_owned(), value));
            }
        }
        if let Ok(id) = std::env::var("FLATPAK_ID") {
            session.push(("Sandbox".to_owned(), format!("Flatpak ({id})")));
        } else if std::env::var_os("APPIMAGE").is_some() {
            session.push(("Sandbox".to_owned(), "AppImage".to_owned()));
        }
        for tool in ["tesseract", "wl-copy", "xclip", "notify-send"] {
            tools.push((tool.to_owned(), on_path(tool)));
        }
    }
    let monitors = vixeeny_platform::monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            format!(
                "{}x{} at {}%{}",
                m.rect.width,
                m.rect.height,
                u64::from(m.dpi) * 100 / 96,
                if m.hdr.is_some() { ", HDR" } else { "" }
            )
        })
        .collect();
    let (gpus, encoders) = probe.map_or_else(Default::default, |p| {
        let registry = vixeeny_encode::registry::Registry::builtin().ok();
        (
            p.adapters
                .iter()
                .filter(|a| !a.software)
                .map(|a| format!("{} (driver {})", a.name, a.driver_version))
                .collect(),
            p.encoders
                .iter()
                .map(|e| {
                    registry
                        .as_ref()
                        .and_then(|r| r.get(&e.id))
                        .map_or_else(|| e.id.clone(), |r| r.display_name.clone())
                })
                .collect(),
        )
    });
    Info {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        build,
        os: os_name(),
        arch: std::env::consts::ARCH.to_owned(),
        session,
        monitors,
        gpus,
        encoders,
        tools,
        settings: vec![
            ("Language".to_owned(), config.general.language.clone()),
            ("Images".to_owned(), config.image.format.clone()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_lists_every_part_and_nothing_personal() {
        let info = Info {
            version: "0.9.0".into(),
            build: vec!["release".into(), "ffmpeg".into()],
            os: "Debian GNU/Linux 13, kernel 6.12".into(),
            arch: "x86_64".into(),
            session: vec![("Session".into(), "wayland".into())],
            monitors: vec!["2560x1440 at 100%".into(), "1920x1080 at 125%".into()],
            gpus: vec![],
            encoders: vec!["H.264 (VAAPI)".into()],
            tools: vec![("tesseract".into(), true), ("xclip".into(), false)],
            settings: vec![("Language".into(), "auto".into())],
        };
        let text = format(&info);
        assert!(text.starts_with("<details>"));
        assert!(text.contains("Vixeeny   0.9.0 (release, ffmpeg)"));
        assert!(text.contains("Displays  2560x1440 at 100%\n"));
        assert!(text.contains("          1920x1080 at 125%\n"));
        assert!(text.contains("GPUs      none\n"));
        assert!(text.contains("tesseract: found"));
        assert!(text.contains("xclip: missing"));
        assert!(text.trim_end().ends_with("</details>"));
    }

    #[test]
    fn os_release_pretty_name() {
        assert_eq!(
            pretty_name("NAME=Arch\nPRETTY_NAME=\"Arch Linux\"\n").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(pretty_name("ID=x\n"), None);
    }
}
