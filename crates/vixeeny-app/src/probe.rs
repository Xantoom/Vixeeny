// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware encoder probe (plan 6.3). `vixeeny-app --probe` is the child process that asks the
//! GPU drivers what their encoders can do and prints the result as TOML; `--probe-report` runs
//! it (through the cache) and prints a readable report, for the 🧪 check against the vendor
//! documentation.

use std::time::Duration;

use anyhow::Context;
use vixeeny_encode::probe::{Adapter, ProbeResult, cached_or_probe, run_child, vendor_from_pci};
use vixeeny_encode::registry::Registry;

/// How long the child may take before it is killed (the target is under 5 s).
const TIMEOUT: Duration = Duration::from_secs(60);

/// The GPUs of this machine; `index` counts the adapters of the same vendor.
pub fn adapters() -> Vec<Adapter> {
    let mut counts = std::collections::HashMap::<u32, u32>::new();
    vixeeny_platform::gpu_adapters()
        .unwrap_or_default()
        .into_iter()
        .map(|g| {
            let n = counts.entry(g.vendor_id).or_default();
            let index = *n;
            *n += 1;
            Adapter {
                index,
                vendor: vendor_from_pci(g.vendor_id),
                name: g.name,
                vendor_id: g.vendor_id,
                device_id: g.device_id,
                driver_version: g.driver_version,
                software: g.software,
            }
        })
        .collect()
}

/// The child process: probes and prints the result on stdout.
pub fn child() -> anyhow::Result<()> {
    use vixeeny_encode::ffmpeg_probe::FfmpegProber;
    let registry = Registry::builtin()?;
    let prober = FfmpegProber::new(adapters());
    let result =
        vixeeny_encode::probe::probe(&registry, &prober, &vixeeny_encode::probe::identity());
    print!("{}", vixeeny_encode::probe::to_toml(&result)?);
    Ok(())
}

/// The probe result: cached while the GPUs and drivers are the same, else a fresh child run.
pub fn current(force: bool) -> anyhow::Result<ProbeResult> {
    let path = vixeeny_common::paths::hw_cache_file().context("no cache folder")?;
    if force {
        let _ = std::fs::remove_file(&path);
    }
    let exe = std::env::current_exe()?;
    Ok(cached_or_probe(
        &path,
        &adapters(),
        &vixeeny_encode::probe::identity(),
        || run_child(&exe, TIMEOUT),
    )?)
}

/// The probe result if the cache already holds it (no process is started).
pub fn cached() -> Option<ProbeResult> {
    let path = vixeeny_common::paths::hw_cache_file()?;
    let key = vixeeny_encode::probe::cache_key(&adapters(), &vixeeny_encode::probe::identity());
    vixeeny_encode::probe::load_cache(&path, &key)
}

/// `--warm-probe`: fills the cache when it is missing, so the first recording and the settings
/// find the answer ready. The daemon starts it in the background at launch.
pub fn warm() -> anyhow::Result<()> {
    current(false).map(|_| ())
}

/// `--probe-report [--force]`: a readable summary.
pub fn report(force: bool) -> anyhow::Result<()> {
    vixeeny_platform::attach_console();
    let started = std::time::Instant::now();
    let result = current(force)?;
    println!("Probe done in {:.1?}\n", started.elapsed());
    println!("GPUs:");
    for a in &result.adapters {
        println!(
            "  [{}] {} — vendor {:04x}, device {:04x}, driver {}{}",
            a.index,
            a.name,
            a.vendor_id,
            a.device_id,
            a.driver_version,
            if a.software {
                " (software renderer, not used)"
            } else {
                ""
            }
        );
    }
    println!("\nEncoders:");
    let registry = Registry::builtin()?;
    for e in &result.encoders {
        let name = registry
            .get(&e.id)
            .map_or(e.id.as_str(), |r| r.display_name.as_str());
        let on = e
            .adapter
            .map_or(String::new(), |i| format!(" (GPU index {i})"));
        println!("  {name}{on}");
        for f in &e.formats {
            println!(
                "      {}-bit {}{}{}",
                f.depth,
                f.chroma.name(),
                if f.uhd { "  4K" } else { "" },
                if f.hdr { "  HDR" } else { "" }
            );
        }
    }
    Ok(())
}
