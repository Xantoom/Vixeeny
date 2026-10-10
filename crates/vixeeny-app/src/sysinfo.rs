// SPDX-License-Identifier: GPL-3.0-or-later
//! The system info (plan M23): the block a bug report starts with ("Report a problem"). It holds what the
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
    pub monitors: Vec<String>,
    pub gpus: Vec<String>,
    pub encoders: Vec<String>,
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
    for (key, value) in &info.settings {
        let _ = writeln!(out, "{key:<9} {value}");
    }
    out.push_str("```\n\n</details>\n");
    out
}

fn os_name() -> String {
    vixeeny_platform::os_version()
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
    if cfg!(feature = "native-codecs") {
        build.push("native-codecs".to_owned());
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
        monitors,
        gpus,
        encoders,
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
            os: "Microsoft Windows [version 10.0.26100.4652]".into(),
            arch: "x86_64".into(),
            monitors: vec!["2560x1440 at 100%".into(), "1920x1080 at 125%".into()],
            gpus: vec![],
            encoders: vec!["H.264 (NVENC)".into()],
            settings: vec![("Language".into(), "auto".into())],
        };
        let text = format(&info);
        assert!(text.starts_with("<details>"));
        assert!(text.contains("Vixeeny   0.9.0 (release, ffmpeg)"));
        assert!(text.contains("Displays  2560x1440 at 100%\n"));
        assert!(text.contains("          1920x1080 at 125%\n"));
        assert!(text.contains("GPUs      none\n"));
        assert!(text.trim_end().ends_with("</details>"));
    }
}
