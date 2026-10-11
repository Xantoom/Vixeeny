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

/// What a tile shows: its icon (or, for a screen, its outline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    Cpu,
    Gpu,
    Ram,
    Screen,
    Disk,
}

/// One card of the general page: a figure or a name, and a short line under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub kind: TileKind,
    pub title: String,
    pub detail: String,
    /// What the part is, while the pointer rests on the card (`Processor`, `Screen 1 (main)`).
    pub tip: String,
    /// A disk's used share, 0 to 1.
    pub used: Option<f32>,
    /// A screen's width over its height.
    pub aspect: f32,
    /// A screen's number, the main one stands out.
    pub badge: String,
    pub primary: bool,
}

impl Tile {
    fn new(kind: TileKind, title: String, detail: String, tip: String) -> Self {
        Self {
            kind,
            title,
            detail,
            tip,
            used: None,
            aspect: 0.0,
            badge: String::new(),
            primary: false,
        }
    }
}

/// The cards of the general page: the parts, the screens, the disks (each empty until the host
/// has read the machine).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overview {
    pub parts: Vec<Tile>,
    pub screens: Vec<Tile>,
    pub disks: Vec<Tile>,
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

/// The cards of the general page.
pub fn overview(env: &Env) -> Overview {
    let m = &env.machine;
    let lang = env.lang;
    let t = |k| tr(k, lang).to_owned();
    let mut o = Overview::default();
    if let Some(cpu) = &m.cpu {
        let cores = t(Key::PcCores)
            .replace("{cores}", &cpu.cores.to_string())
            .replace("{threads}", &cpu.threads.to_string());
        o.parts.push(Tile::new(
            TileKind::Cpu,
            short_name(&cpu.name),
            cores,
            t(Key::PcCpu),
        ));
    }
    for gpu in &m.gpus {
        let (tip, memory) = if gpu.integrated {
            (t(Key::PcIgpu), String::new())
        } else if gpu.memory_bytes > 0 {
            (t(Key::PcGpu), memory(gpu.memory_bytes, lang))
        } else {
            (t(Key::PcGpu), String::new())
        };
        o.parts
            .push(Tile::new(TileKind::Gpu, short_name(&gpu.name), memory, tip));
    }
    if m.ram_bytes > 0 {
        o.parts.push(Tile::new(
            TileKind::Ram,
            memory(m.ram_bytes, lang),
            String::new(),
            t(Key::PcRam),
        ));
    }
    for s in &m.screens {
        let mut tip = t(Key::PcScreenN).replace("{n}", &s.number.to_string());
        if s.primary {
            tip = format!("{tip} ({})", t(Key::PcPrimary));
        }
        let mut tile = Tile::new(
            TileKind::Screen,
            format!("{} × {}", s.width, s.height),
            format!("{} Hz", s.hz),
            joined(&[&tip, &s.name]),
        );
        tile.aspect = if s.height > 0 {
            s.width as f32 / s.height as f32
        } else {
            16.0 / 9.0
        };
        tile.badge = s.number.to_string();
        tile.primary = s.primary;
        o.screens.push(tile);
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
        let mut tile = Tile::new(
            TileKind::Disk,
            d.letter.clone(),
            free,
            joined(&[&t(Key::PcDriveN).replace("{letter}", &d.letter), name]),
        );
        tile.used =
            (d.bytes > 0).then(|| 1.0 - (d.free_bytes.min(d.bytes) as f64 / d.bytes as f64) as f32);
        o.disks.push(tile);
    }
    o
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
        let shown = |tiles: &[Tile]| {
            tiles
                .iter()
                .map(|t| format!("{} | {} | {}", t.title, t.detail, t.tip))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(&o.parts),
            [
                "AMD Ryzen 7 9800X3D | 8 cores, 16 threads | Processor",
                "NVIDIA GeForce RTX 5080 | 16 GB | Graphics card",
                "AMD Radeon Graphics |  | Integrated graphics",
                "64 GB |  | Memory (RAM)",
            ]
        );
        assert_eq!(
            shown(&o.screens),
            [
                "3840 × 2160 | 240 Hz | Screen 1 (main) · Odyssey G80SH",
                "3440 × 1440 | 144 Hz | Screen 2 · LG ULTRAWIDE",
            ]
        );
        assert!(o.screens[0].primary && !o.screens[1].primary);
        assert!((o.screens[1].aspect - 2.389).abs() < 0.01);
        assert_eq!(
            shown(&o.disks),
            ["C: | 1 TB free of 2 TB | Drive C: · CT2000T500SSD8"]
        );
        assert!((o.disks[0].used.unwrap() - 0.5).abs() < 0.01);
        assert_eq!(overview(&Env::new(Lang::En, "1.0.0")), Overview::default());
    }
}
