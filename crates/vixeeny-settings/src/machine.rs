// SPDX-License-Identifier: GPL-3.0-or-later
//! What this computer is made of, as the top of the general page shows it (the host reads it
//! from the OS): a tile per part, grouped as the parts, the screens and the disks.

use vixeeny_common::i18n::{Key, Lang, tr};

pub use vixeeny_common::machine::{Cpu, Disk, Gpu, Machine, Module, Screen};

use crate::Env;

/// What a tile shows: its icon (or, for a screen, its outline). Its number is the `kind` of a
/// `PcTile`.
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
    /// What the part is, for screen readers (`Processor`, `Screen 1 (main)`).
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

/// The cards of the general page, a row each: processor and memory, graphics, screens, disks
/// (each empty until the host has read the machine).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overview {
    pub system: Vec<Tile>,
    pub gpus: Vec<Tile>,
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

/// `4.7 GHz` (`4,7 GHz` in French).
fn ghz(mhz: u32, lang: Lang) -> String {
    let text = format!("{:.1}", f64::from(mhz) / 1000.0);
    let text = if lang == Lang::Fr {
        text.replace('.', ",")
    } else {
        text
    };
    format!("{text} GHz")
}

/// `2 × 32 GB · DDR5 6000 MT/s CL30`: the sticks, their kind, speed and latency, as far as known.
fn sticks(modules: &[Module], lang: Lang) -> String {
    let Some(first) = modules.first() else {
        return String::new();
    };
    let same = |f: fn(&Module) -> u64| modules.iter().all(|m| f(m) == f(first));
    let sizes = if same(|m| m.bytes) {
        format!("{} × {}", modules.len(), memory(first.bytes, lang))
    } else {
        modules
            .iter()
            .map(|m| memory(m.bytes, lang))
            .collect::<Vec<_>>()
            .join(" + ")
    };
    let mut kind = Vec::new();
    if modules.iter().all(|m| m.kind == first.kind) && !first.kind.is_empty() {
        kind.push(first.kind.clone());
    }
    // The slowest stick sets the pace.
    let mts = modules.iter().map(|m| m.mts).min().unwrap_or(0);
    if mts > 0 {
        kind.push(format!("{mts} MT/s"));
    }
    if same(|m| u64::from(m.cl)) && first.cl > 0 {
        kind.push(format!("CL{}", first.cl));
    }
    joined(&[&sizes, &kind.join(" ")])
}

/// The shape of a screen when it is not the usual 16:9 (`21:9`, `16:10`, `9:16` upright).
fn aspect(width: u32, height: u32) -> Option<String> {
    if width == 0 || height == 0 {
        return None;
    }
    let (long, short) = (width.max(height), width.min(height));
    let r = f64::from(long) / f64::from(short);
    let known = [
        (1.25, "5:4"),
        (4.0 / 3.0, "4:3"),
        (1.5, "3:2"),
        (1.6, "16:10"),
        (16.0 / 9.0, "16:9"),
        (2.0, "18:9"),
        (2.37, "21:9"),
        (3.2, "32:10"),
        (32.0 / 9.0, "32:9"),
    ];
    let name = known
        .iter()
        .find(|(k, _)| (r - k).abs() / k < 0.03)
        .map_or_else(
            || {
                let (mut a, mut b) = (long, short);
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                format!("{}:{}", long / a, short / a)
            },
            |(_, n)| (*n).to_owned(),
        );
    if name == "16:9" && width > height {
        return None;
    }
    Some(if height > width {
        let (w, h) = name.split_once(':').unwrap_or((&name, ""));
        format!("{h}:{w}")
    } else {
        name
    })
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
        let clock = match (cpu.base_mhz, cpu.boost_mhz) {
            (0, _) => String::new(),
            (base, boost) if boost > base => {
                format!("{} (boost {})", ghz(base, lang), ghz(boost, lang))
            }
            (base, _) => ghz(base, lang),
        };
        o.system.push(Tile::new(
            TileKind::Cpu,
            short_name(&cpu.name),
            joined(&[&cores, &clock]),
            t(Key::PcCpu),
        ));
    }
    if m.ram_bytes > 0 {
        o.system.push(Tile::new(
            TileKind::Ram,
            memory(m.ram_bytes, lang),
            sticks(&m.modules, lang),
            t(Key::PcRam),
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
        o.gpus
            .push(Tile::new(TileKind::Gpu, short_name(&gpu.name), memory, tip));
    }
    for s in &m.screens {
        let mut title = format!("{} × {}", s.width, s.height);
        if let Some(shape) = aspect(s.width, s.height) {
            title = format!("{title} ({shape})");
        }
        let mut detail = format!("{} Hz", s.hz);
        if s.primary {
            detail = format!("{detail} ({})", t(Key::PcPrimary));
        }
        let mut tile = Tile::new(
            TileKind::Screen,
            title,
            detail,
            joined(&[
                &t(Key::PcScreenN).replace("{n}", &s.number.to_string()),
                &s.name,
            ]),
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
        // As the Explorer names it: `Games (D:)`, `Local Disk (C:)`.
        let name = if d.label.is_empty() {
            t(Key::PcLocalDisk)
        } else {
            d.label.clone()
        };
        let free = t(Key::PcFree)
            .replace("{free}", &capacity(d.free_bytes, lang))
            .replace("{total}", &capacity(d.bytes, lang));
        let mut tile = Tile::new(
            TileKind::Disk,
            format!("{name} ({})", d.letter),
            free,
            d.model.clone(),
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
    fn screens_say_their_shape_unless_16_9() {
        assert_eq!(aspect(3840, 2160), None);
        assert_eq!(aspect(3440, 1440).as_deref(), Some("21:9"));
        assert_eq!(aspect(2560, 1080).as_deref(), Some("21:9"));
        assert_eq!(aspect(5120, 1440).as_deref(), Some("32:9"));
        assert_eq!(aspect(1920, 1200).as_deref(), Some("16:10"));
        assert_eq!(aspect(1080, 1920).as_deref(), Some("9:16"));
        assert_eq!(aspect(1280, 1024).as_deref(), Some("5:4"));
    }

    #[test]
    fn the_general_page_shows_each_part() {
        let stick = |cl| Module {
            bytes: 32 << 30,
            kind: "DDR5".into(),
            mts: 6000,
            cl,
        };
        let env = Env::new(Lang::En, "1.0.0").with_machine(Machine::sample());
        let o = overview(&env);
        let shown = |tiles: &[Tile]| {
            tiles
                .iter()
                .map(|t| format!("{} | {} | {}", t.title, t.detail, t.tip))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(&o.system),
            [
                "AMD Ryzen 7 9800X3D | 8 cores, 16 threads · 4.7 GHz | Processor",
                "64 GB | 2 × 32 GB · DDR5 6000 MT/s CL30 | Memory (RAM)",
            ]
        );
        assert_eq!(
            shown(&o.gpus),
            ["NVIDIA GeForce RTX 5080 | 16 GB | Graphics card"]
        );
        assert_eq!(
            shown(&o.screens),
            [
                "3840 × 2160 | 240 Hz (main) | Screen 1 · Odyssey G80SH",
                "3440 × 1440 (21:9) | 144 Hz | Screen 2 · LG ULTRAWIDE",
            ]
        );
        assert!(o.screens[0].primary && !o.screens[1].primary);
        assert!((o.screens[1].aspect - 2.389).abs() < 0.01);
        assert_eq!(
            shown(&o.disks),
            [
                "Local Disk (C:) | 1 TB free of 2 TB | CT2000T500SSD8",
                "Games (D:) | 100 GB free of 4 TB | ",
            ]
        );
        assert!((o.disks[0].used.unwrap() - 0.5).abs() < 0.01);
        // An Intel processor tells its boost; sticks of mixed sizes are listed one by one.
        let mut m = env.machine;
        if let Some(c) = m.cpu.as_mut() {
            (c.base_mhz, c.boost_mhz) = (3600, 5800);
        }
        m.modules = vec![
            stick(0),
            Module {
                bytes: 16 << 30,
                ..stick(0)
            },
        ];
        let o = overview(&Env::new(Lang::Fr, "1.0.0").with_machine(m));
        assert!(o.system[0].detail.ends_with("3,6 GHz (boost 5,8 GHz)"));
        assert_eq!(o.system[1].detail, "32 Go + 16 Go · DDR5 6000 MT/s");
        assert_eq!(overview(&Env::new(Lang::En, "1.0.0")), Overview::default());
    }
}
