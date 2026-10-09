// SPDX-License-Identifier: GPL-3.0-or-later
//! What this computer is made of, as the general page shows it (the host reads it from the OS).

use vixeeny_common::i18n::{Key, Lang};

use crate::{Env, Row, header, hinted, info};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub model: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub hz: u32,
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

/// The rows of the general page about this computer (none until the host has read it).
pub fn rows(env: &Env) -> Vec<Row> {
    let m = &env.machine;
    let lang = env.lang;
    let t = |k| env.t(k);
    let mut rows = Vec::new();
    let mut add = |id: String, label: Key, value: String, hint: String| {
        let row = info(&id, t(label), value);
        rows.push(if hint.is_empty() {
            row
        } else {
            hinted(row, hint)
        });
    };
    if let Some(cpu) = &m.cpu {
        let cores = t(Key::PcCores)
            .replace("{cores}", &cpu.cores.to_string())
            .replace("{threads}", &cpu.threads.to_string());
        add("pc_cpu".into(), Key::PcCpu, cpu.name.clone(), cores);
    }
    if m.ram_bytes > 0 {
        add(
            "pc_ram".into(),
            Key::PcRam,
            memory(m.ram_bytes, lang),
            String::new(),
        );
    }
    for (i, gpu) in m.gpus.iter().enumerate() {
        let (label, hint) = if gpu.integrated {
            (Key::PcIgpu, String::new())
        } else {
            let video = t(Key::PcVram).replace("{size}", &memory(gpu.memory_bytes, lang));
            (
                Key::PcGpu,
                if gpu.memory_bytes > 0 {
                    video
                } else {
                    String::new()
                },
            )
        };
        add(format!("pc_gpu:{i}"), label, gpu.name.clone(), hint);
    }
    for (i, disk) in m.disks.iter().enumerate() {
        add(
            format!("pc_disk:{i}"),
            Key::PcDisk,
            disk.model.clone(),
            capacity(disk.bytes, lang),
        );
    }
    for (i, screen) in m.screens.iter().enumerate() {
        let mode = format!("{} × {}, {} Hz", screen.width, screen.height, screen.hz);
        add(
            format!("pc_screen:{i}"),
            Key::PcScreen,
            screen.name.clone(),
            mode,
        );
    }
    if !rows.is_empty() {
        rows.insert(0, header("h_pc"));
    }
    rows
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
    fn the_general_page_lists_each_part() {
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
                model: "CT2000T500SSD8".into(),
                bytes: 2_000_398_934_016,
            }],
            screens: vec![Screen {
                name: "Odyssey G80SH".into(),
                width: 3840,
                height: 2160,
                hz: 240,
            }],
        });
        let rows = rows(&env);
        let line = |id: &str| {
            let r = rows
                .iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id}"));
            (r.label.clone(), r.hint.clone())
        };
        assert_eq!(line("pc_cpu").1, "8 cores, 16 threads");
        assert_eq!(line("pc_gpu:0").1, "16 GB of video memory");
        assert_eq!(line("pc_gpu:1").0, "Integrated graphics");
        assert_eq!(line("pc_disk:0").1, "2 TB");
        assert_eq!(line("pc_screen:0").1, "3840 × 2160, 240 Hz");
        assert!(super::rows(&Env::new(Lang::En, "1.0.0")).is_empty());
    }
}
