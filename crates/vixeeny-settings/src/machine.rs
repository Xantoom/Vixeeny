// SPDX-License-Identifier: GPL-3.0-or-later
//! What this computer is made of, as the top of the general page shows it (the host reads it
//! from the OS): a tile per part, grouped as the parts, the screens and the disks.

use vixeeny_common::i18n::{Key, Lang, tr};

use crate::Env;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub cpu: Option<Cpu>,
    /// Installed memory, in bytes (0: unknown).
    pub ram_bytes: u64,
    /// The graphics cards first, then the processor's graphics.
    pub gpus: Vec<Gpu>,
    pub disks: Vec<Disk>,
    pub screens: Vec<Screen>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpu {
    pub name: String,
    pub cores: u32,
    pub threads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    pub integrated: bool,
    /// Its own memory, in bytes (0 for an integrated one).
    pub memory_bytes: u64,
}

/// A drive letter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    /// `C:`.
    pub letter: String,
    /// The name given to the volume (often empty).
    pub label: String,
    /// The model of the disk under it (empty when unknown).
    pub model: String,
    pub bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// 1, 2, 3...
    pub number: u32,
    pub primary: bool,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

/// One line of the sheet: what the part is, and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// `Processor`, `Screen 2`.
    pub label: String,
    /// Its name and figures, on one line.
    pub value: String,
}

/// A size the way the OS shows memory: binary gigabytes, whole.
fn memory(bytes: u64, lang: Lang) -> String {
    let gb = (bytes as f64 / f64::from(1u32 << 30)).round();
    format!("{gb} {}", if lang == Lang::Fr { "Go" } else { "GB" })
}

/// A size the way disks are sold: decimal, terabytes with two decimals at most.
fn capacity(bytes: u64, lang: Lang) -> String {
    let fr = lang == Lang::Fr;
    let (value, unit) = if bytes >= 1_000_000_000_000 {
        (bytes as f64 / 1e12, if fr { "To" } else { "TB" })
    } else {
        ((bytes as f64 / 1e9).round(), if fr { "Go" } else { "GB" })
    };
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    let text = if fr {
        text.replace('.', ",")
    } else {
        text.to_owned()
    };
    format!("{text} {unit}")
}

/// A part's name without what the box already says ("8-Core Processor", "(TM)").
fn short_name(name: &str) -> String {
    let mut words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case("processor") && !w.ends_with("-Core"))
        .collect();
    if words.last().is_some_and(|w| w.eq_ignore_ascii_case("cpu")) {
        words.pop();
    }
    words
        .join(" ")
        .replace("(TM)", "")
        .replace("(R)", "")
        .replace("(tm)", "")
        .replace("(r)", "")
}

fn joined(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The sheet of the general page, part by part (empty until the host has read the machine).
pub fn overview(env: &Env) -> Vec<Line> {
    let m = &env.machine;
    let lang = env.lang;
    let t = |k| tr(k, lang).to_owned();
    let mut lines = Vec::new();
    let mut line = |label: String, value: String| lines.push(Line { label, value });
    if let Some(cpu) = &m.cpu {
        let cores = t(Key::PcCores)
            .replace("{cores}", &cpu.cores.to_string())
            .replace("{threads}", &cpu.threads.to_string());
        line(t(Key::PcCpu), joined(&[&short_name(&cpu.name), &cores]));
    }
    for gpu in &m.gpus {
        let (label, memory) = if gpu.integrated {
            (t(Key::PcIgpu), String::new())
        } else if gpu.memory_bytes > 0 {
            (t(Key::PcGpu), memory(gpu.memory_bytes, lang))
        } else {
            (t(Key::PcGpu), String::new())
        };
        line(label, joined(&[&short_name(&gpu.name), &memory]));
    }
    if m.ram_bytes > 0 {
        line(t(Key::PcRam), memory(m.ram_bytes, lang));
    }
    for s in &m.screens {
        let mut label = t(Key::PcScreenN).replace("{n}", &s.number.to_string());
        if s.primary {
            label = format!("{label} ({})", t(Key::PcPrimary));
        }
        let size = format!("{}×{}", s.width, s.height);
        line(label, joined(&[&s.name, &size, &format!("{} Hz", s.hz)]));
    }
    for d in &m.disks {
        let name = if d.label.is_empty() {
            &d.model
        } else {
            &d.label
        };
        let free = t(Key::PcFree)
            .replace("{free}", &capacity(d.free_bytes, lang))
            .replace("{total}", &capacity(d.bytes, lang));
        line(
            t(Key::PcDriveN).replace("{letter}", &d.letter),
            joined(&[name, &free]),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_the_os_and_the_box_say() {
        assert_eq!(memory(68_719_476_736, Lang::Fr), "64 Go");
        assert_eq!(memory(16_753_098_752, Lang::En), "16 GB");
        assert_eq!(capacity(2_000_398_934_016, Lang::Fr), "2 To");
        assert_eq!(capacity(1_920_383_410_176, Lang::Fr), "1,92 To");
        assert_eq!(capacity(1_920_383_410_176, Lang::En), "1.92 TB");
        assert_eq!(capacity(512_110_190_592, Lang::En), "512 GB");
    }

    #[test]
    fn the_general_page_shows_each_part() {
        let env = Env::new(Lang::En, "1.0.0").with_machine(Machine {
            cpu: Some(Cpu {
                name: "AMD Ryzen 7 9800X3D 8-Core Processor".into(),
                cores: 8,
                threads: 16,
            }),
            ram_bytes: 64 << 30,
            gpus: vec![
                Gpu {
                    name: "NVIDIA GeForce RTX 5080".into(),
                    integrated: false,
                    memory_bytes: 16 << 30,
                },
                Gpu {
                    name: "AMD Radeon(TM) Graphics".into(),
                    integrated: true,
                    memory_bytes: 0,
                },
            ],
            disks: vec![Disk {
                letter: "C:".into(),
                label: String::new(),
                model: "CT2000T500SSD8".into(),
                bytes: 2_000_398_934_016,
                free_bytes: 1_000_000_000_000,
            }],
            screens: vec![
                Screen {
                    number: 1,
                    primary: true,
                    name: "Odyssey G80SH".into(),
                    width: 3840,
                    height: 2160,
                    hz: 240,
                },
                Screen {
                    number: 2,
                    primary: false,
                    name: "LG ULTRAWIDE".into(),
                    width: 3440,
                    height: 1440,
                    hz: 144,
                },
            ],
        });
        let o = overview(&env);
        let value = |label: &str| {
            o.iter()
                .find(|l| l.label == label)
                .map(|l| l.value.clone())
                .unwrap_or_else(|| panic!("{label}"))
        };
        assert_eq!(
            value("Processor"),
            "AMD Ryzen 7 9800X3D · 8 cores, 16 threads"
        );
        assert_eq!(value("Graphics card"), "NVIDIA GeForce RTX 5080 · 16 GB");
        assert_eq!(value("Integrated graphics"), "AMD Radeon Graphics");
        assert_eq!(value("Memory (RAM)"), "64 GB");
        assert_eq!(
            value("Screen 1 (main)"),
            "Odyssey G80SH · 3840×2160 · 240 Hz"
        );
        assert_eq!(value("Screen 2"), "LG ULTRAWIDE · 3440×1440 · 144 Hz");
        assert_eq!(value("Drive C:"), "CT2000T500SSD8 · 1 TB free of 2 TB");
        assert!(overview(&Env::new(Lang::En, "1.0.0")).is_empty());
    }
}
