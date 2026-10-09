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

/// What a tile shows in its corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Cpu,
    Ram,
    Gpu,
    /// The drive letter, as text.
    Drive,
    /// A small screen of the screen's shape, its number inside.
    Screen,
}

/// One part of the computer.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub glyph: Glyph,
    /// What the part is (`Processor`, `Screen 2`).
    pub caption: String,
    /// Its name.
    pub title: String,
    /// One line of figures.
    pub detail: String,
    /// A word beside the caption (`Main`), empty for none.
    pub badge: String,
    /// The disk's letter, the screen's number.
    pub mark: String,
    /// How full a disk is (0 to 1), `None` for the rest.
    pub usage: Option<f32>,
    /// A screen's width / height.
    pub ratio: f32,
}

/// The tiles of the general page, in three groups: parts, screens, disks (all empty until the
/// host has read the machine).
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

fn tile(glyph: Glyph, caption: String, title: String, detail: String) -> Tile {
    Tile {
        glyph,
        caption,
        title,
        detail,
        badge: String::new(),
        mark: String::new(),
        usage: None,
        ratio: 16.0 / 9.0,
    }
}

pub fn overview(env: &Env) -> Overview {
    let m = &env.machine;
    let lang = env.lang;
    let t = |k| tr(k, lang).to_owned();
    let mut parts = Vec::new();
    if let Some(cpu) = &m.cpu {
        let cores = t(Key::PcCores)
            .replace("{cores}", &cpu.cores.to_string())
            .replace("{threads}", &cpu.threads.to_string());
        parts.push(tile(Glyph::Cpu, t(Key::PcCpu), cpu.name.clone(), cores));
    }
    for gpu in &m.gpus {
        let (caption, detail) = if gpu.integrated {
            (t(Key::PcIgpu), t(Key::PcShared))
        } else if gpu.memory_bytes > 0 {
            (
                t(Key::PcGpu),
                t(Key::PcVram).replace("{size}", &memory(gpu.memory_bytes, lang)),
            )
        } else {
            (t(Key::PcGpu), String::new())
        };
        parts.push(tile(Glyph::Gpu, caption, gpu.name.clone(), detail));
    }
    if m.ram_bytes > 0 {
        parts.push(tile(
            Glyph::Ram,
            t(Key::PcRam),
            memory(m.ram_bytes, lang),
            t(Key::PcInstalled),
        ));
    }
    let screens = m
        .screens
        .iter()
        .map(|s| {
            let mut tile = tile(
                Glyph::Screen,
                t(Key::PcScreenN).replace("{n}", &s.number.to_string()),
                s.name.clone(),
                format!("{}×{} · {} Hz", s.width, s.height, s.hz),
            );
            if s.primary {
                tile.badge = t(Key::PcPrimary);
            }
            tile.mark = s.number.to_string();
            tile.ratio = s.width as f32 / s.height.max(1) as f32;
            tile
        })
        .collect();
    let disks = m
        .disks
        .iter()
        .map(|d| {
            let name = match (d.label.is_empty(), d.model.is_empty()) {
                (false, _) => d.label.clone(),
                (true, false) => d.model.clone(),
                (true, true) => t(Key::PcDisk),
            };
            let detail = t(Key::PcFree)
                .replace("{free}", &capacity(d.free_bytes, lang))
                .replace("{total}", &capacity(d.bytes, lang));
            let mut tile = tile(
                Glyph::Drive,
                t(Key::PcDriveN).replace("{letter}", &d.letter),
                name,
                detail,
            );
            tile.mark = d.letter.clone();
            tile.usage = (d.bytes > 0)
                .then(|| (d.bytes.saturating_sub(d.free_bytes)) as f32 / d.bytes as f32);
            tile
        })
        .collect();
    Overview {
        parts,
        screens,
        disks,
    }
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
        assert_eq!(o.parts[0].detail, "8 cores, 16 threads");
        assert_eq!(o.parts[1].detail, "16 GB of video memory");
        assert_eq!(o.parts[2].caption, "Integrated graphics");
        assert_eq!(o.parts[3].title, "64 GB");
        assert_eq!(o.disks[0].caption, "Drive C:");
        assert_eq!(o.disks[0].title, "CT2000T500SSD8");
        assert_eq!(o.disks[0].detail, "1 TB free of 2 TB");
        assert!(o.disks[0].usage.is_some_and(|u| (u - 0.5).abs() < 0.01));
        assert_eq!(o.screens[0].caption, "Screen 1");
        assert_eq!(o.screens[0].badge, "Main");
        assert_eq!(o.screens[0].detail, "3840×2160 · 240 Hz");
        assert!(o.screens[1].badge.is_empty());
        assert!(o.screens[1].ratio > 2.3);
        assert_eq!(overview(&Env::new(Lang::En, "1.0.0")), Overview::default());
    }
}
