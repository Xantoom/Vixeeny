// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask dist [--out <dir>]` (Windows release layout, plan 10.1) and
//! `cargo xtask sums <dir>`.
//!
//! `dist` expects `cargo build --release` to have produced the three programs, and gathers them
//! with the FFmpeg DLLs and the licences into `<out>/stage` (what the installer packs), then
//! writes the update archive `Vixeeny-<version>-windows-x64.zip` and the portable archive
//! `Vixeeny-<version>-windows-x64-portable.zip` (the same files plus `portable.flag`).

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const PROGRAMS: [&str; 3] = [
    "vixeeny-daemon.exe",
    "vixeeny-app.exe",
    "vixeeny-updater.exe",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every file under `dir`, relative, sorted.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(sub) = pending.pop() {
        for entry in std::fs::read_dir(dir.join(&sub))? {
            let entry = entry?;
            let relative = sub.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                pending.push(relative);
            } else {
                found.push(relative);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn write_zip(stage: &Path, extra: &[(&str, &[u8])], out: &Path) -> Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(out)?);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for relative in files_under(stage)? {
        let name = relative.to_string_lossy().replace('\\', "/");
        zip.start_file(name, options)?;
        zip.write_all(&std::fs::read(stage.join(&relative))?)?;
    }
    for (name, data) in extra {
        zip.start_file(*name, options)?;
        zip.write_all(data)?;
    }
    zip.finish()?;
    Ok(())
}

/// The libraries of the build with their licences: the crates (from `cargo metadata`) and the
/// native libraries (from `native/versions.toml`).
pub fn third_party_licenses() -> Result<String> {
    let root = root();
    let output = std::process::Command::new("cargo")
        .current_dir(&root)
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()
        .context("cargo metadata")?;
    if !output.status.success() {
        bail!("cargo metadata failed");
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let mut lines = vec![];
    for package in metadata["packages"].as_array().into_iter().flatten() {
        if package["source"].is_null() {
            continue; // the workspace's own crates
        }
        let field = |k: &str| package[k].as_str().unwrap_or("?");
        lines.push(format!(
            "{} {} — {}",
            field("name"),
            field("version"),
            package["license"].as_str().unwrap_or("see the crate")
        ));
    }
    lines.sort();
    lines.dedup();
    let mut text = String::from(
        "Vixeeny is free software under the GNU GPL v3 or later (see LICENSE).\n\n\
         Native libraries (source code: see the releases and native/versions.toml):\n",
    );
    let versions: toml::Table = std::fs::read_to_string(root.join("native/versions.toml"))?
        .parse()
        .context("native/versions.toml")?;
    for (name, entry) in &versions {
        if let Some(license) = entry.get("license").and_then(toml::Value::as_str) {
            let tag = entry.get("tag").and_then(toml::Value::as_str).unwrap_or("");
            let _ = writeln!(text, "{name} {tag} — {license}");
        }
    }
    text.push_str("\nRust crates:\n");
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

pub fn dist(args: &[String]) -> Result<()> {
    let root = root();
    let mut out = root.join("dist");
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => out = PathBuf::from(it.next().context("--out needs a folder")?),
            other => bail!("unknown option `{other}`"),
        }
    }
    let version = env!("CARGO_PKG_VERSION");
    let stage = out.join("stage");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let release = root.join("target/release");
    for program in PROGRAMS {
        std::fs::copy(release.join(program), stage.join(program))
            .with_context(|| format!("{program} is missing: run `cargo build --release` first"))?;
    }
    let ffmpeg_bin = root.join("native/build/work/ffmpeg-prebuilt/bin");
    if let Ok(entries) = std::fs::read_dir(&ffmpeg_bin) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
            {
                std::fs::copy(&path, stage.join(entry.file_name()))?;
            }
        }
    }
    std::fs::copy(root.join("LICENSE"), stage.join("LICENSE"))?;
    std::fs::write(
        stage.join("THIRD-PARTY-LICENSES.txt"),
        third_party_licenses()?,
    )?;

    let base = format!("Vixeeny-{version}-windows-x64");
    write_zip(&stage, &[], &out.join(format!("{base}.zip")))?;
    write_zip(
        &stage,
        &[("portable.flag", b"")],
        &out.join(format!("{base}-portable.zip")),
    )?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Writes `SHA256SUMS` (`<hex>  <name>`) for the files of `dir` (not the sums file itself).
pub fn sums(args: &[String]) -> Result<()> {
    let dir = PathBuf::from(args.first().context("usage: cargo xtask sums <dir>")?);
    let mut text = String::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_file() || name == "SHA256SUMS" || name.ends_with(".minisig") {
            continue;
        }
        let hash = Sha256::digest(std::fs::read(entry.path())?);
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(text, "{hex}  {name}");
    }
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_by_key(|l| l.split_once("  ").map(|(_, n)| n.to_owned()));
    std::fs::write(dir.join("SHA256SUMS"), lines.join("\n") + "\n")?;
    Ok(())
}
