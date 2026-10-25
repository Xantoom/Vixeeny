// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask dist [--out <dir>]` (Windows release layout).
//!
//! `dist` expects `cargo build --release` to have produced the two programs, and gathers them with the FFmpeg DLLs and
//! the licences into `<out>/stage` (what the installer packs), then writes the update archive
//! `Vixeeny-<version>-windows-x64.zip` and the portable archive `Vixeeny-<version>-windows-x64-portable.zip` (the same
//! files plus `portable.flag`).

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const PROGRAMS: [&str; 2] = ["Vixeeny.exe", "vixeeny-app.exe"];

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

/// The list of the files of a version: the next update removes those it no longer has (`vixeeny-updater`'s
/// `swap::MANIFEST`).
const MANIFEST: &str = "files.txt";

fn manifest(stage: &Path) -> Result<String> {
    let mut text = String::new();
    for relative in files_under(stage)? {
        writeln!(text, "{}", relative.to_string_lossy().replace('\\', "/"))?;
    }
    Ok(text)
}

/// Each file is compressed on its own thread, in an archive of its own, and the pieces are copied into `out` in order.
fn write_zip(stage: &Path, out: &Path) -> Result<()> {
    use rayon::prelude::*;
    let pieces = files_under(stage)?
        .par_iter()
        .map(|relative| -> Result<_> {
            let mut piece = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            piece.start_file(relative.to_string_lossy().replace('\\', "/"), deflated())?;
            piece.write_all(&std::fs::read(stage.join(relative))?)?;
            Ok(piece.finish_into_readable()?)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(out)?);
    for piece in pieces {
        zip.merge_archive(piece)?;
    }
    zip.finish()?;
    Ok(())
}

fn deflated() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9))
}

/// A copy of the archive `from` with the files `extra` added: what is already compressed is not compressed again.
fn extend_zip(from: &Path, extra: &[(&str, &[u8])], out: &Path) -> Result<()> {
    std::fs::copy(from, out)?;
    let file = std::fs::OpenOptions::new().read(true).write(true).open(out)?;
    let mut zip = zip::ZipWriter::new_append(file)?;
    for (name, data) in extra {
        zip.start_file(*name, deflated())?;
        zip.write_all(data)?;
    }
    zip.finish()?;
    Ok(())
}

/// The libraries of the build with their licences: the crates (from `cargo metadata`) and the native libraries (from
/// `native/versions.toml`).
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
    let versions: toml::Table =
        std::fs::read_to_string(root.join("native/versions.toml"))?.parse().context("native/versions.toml")?;
    for (name, entry) in &versions {
        if let Some(license) = entry.get("license").and_then(toml::Value::as_str) {
            let tag = entry.get("tag").and_then(toml::Value::as_str).unwrap_or("");
            let _ = writeln!(text, "{name} {tag} — {license}");
        }
    }
    text.push_str(
        "\nAssets:\nInter (the typeface of the text drawn on captures) — SIL OFL 1.1\n\
         Fluent UI System Icons (Microsoft; the icons of the windows) — MIT\n\
         Octicons (GitHub; the GitHub mark on the About page) — MIT\n",
    );
    text.push_str("\nRust crates:\n");
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

/// An FFmpeg DLL (`avcodec-63.dll`, `swscale-10.dll`…); not `avrt.dll`, a Windows one.
fn is_ffmpeg(dll: &str) -> bool {
    const LIBRARIES: [&str; 7] = ["avcodec", "avdevice", "avfilter", "avformat", "avutil", "swresample", "swscale"];
    let dll = dll.to_ascii_lowercase();
    dll.ends_with(".dll") && LIBRARIES.iter().any(|lib| dll.strip_prefix(lib).is_some_and(|rest| rest.starts_with('-')))
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
        let imports = crate::pe::imports(&std::fs::read(stage.join(program))?)
            .with_context(|| format!("cannot read the imports of {program}"))?;
        if let Some(dll) = imports.at_start.iter().find(|d| is_ffmpeg(d)) {
            bail!("{program} loads {dll} when it starts: FFmpeg must be delay-loaded (FFMPEG_DIR)");
        }
    }
    let ffmpeg_bin = root.join("native/build/work/ffmpeg-prebuilt/bin");
    if let Ok(entries) = std::fs::read_dir(&ffmpeg_bin) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            // libavdevice (cameras, screen grabbers) is used by no program of Vixeeny.
            if name.ends_with(".dll") && !name.starts_with("avdevice") {
                std::fs::copy(&path, stage.join(entry.file_name()))?;
            }
        }
    }
    std::fs::copy(root.join("LICENSE"), stage.join("LICENSE"))?;
    std::fs::write(stage.join("THIRD-PARTY-LICENSES.txt"), third_party_licenses()?)?;
    std::fs::write(stage.join(MANIFEST), manifest(&stage)?)?;

    let base = format!("Vixeeny-{version}-windows-x64");
    let update = out.join(format!("{base}.zip"));
    write_zip(&stage, &update)?;
    extend_zip(&update, &[("portable.flag", b"")], &out.join(format!("{base}-portable.zip")))?;
    println!("wrote {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_dlls_are_told_from_the_others() {
        assert!(is_ffmpeg("avcodec-63.dll") && is_ffmpeg("SWSCALE-10.DLL"));
        assert!(!is_ffmpeg("avrt.dll") && !is_ffmpeg("KERNEL32.dll"));
        assert!(!is_ffmpeg("avcodec-63.lib"));
    }

    #[test]
    fn the_manifest_lists_every_file_with_forward_slashes() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("vixeeny-manifest-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("licenses"))?;
        std::fs::write(dir.join("Vixeeny.exe"), "")?;
        std::fs::write(dir.join("licenses/a.txt"), "")?;
        let text = manifest(&dir)?;
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(text, "Vixeeny.exe\nlicenses/a.txt\n");
        Ok(())
    }

    #[test]
    fn the_portable_archive_is_the_update_archive_plus_its_flag() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("vixeeny-dist-{}", std::process::id()));
        let stage = dir.join("stage");
        std::fs::create_dir_all(stage.join("sub"))?;
        std::fs::write(stage.join("a.txt"), "a".repeat(10_000))?;
        std::fs::write(stage.join("sub").join("b.txt"), "b")?;
        let (update, portable) = (dir.join("u.zip"), dir.join("p.zip"));
        write_zip(&stage, &update)?;
        extend_zip(&update, &[("portable.flag", b"")], &portable)?;

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&portable)?)?;
        let mut names: Vec<_> =
            zip.file_names().map(|n| n.map(std::borrow::Cow::into_owned)).collect::<Result<_, _>>()?;
        names.sort();
        assert_eq!(names, ["a.txt", "portable.flag", "sub/b.txt"]);
        let mut text = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("a.txt")?, &mut text)?;
        assert_eq!(text, "a".repeat(10_000));
        assert_eq!(zip::ZipArchive::new(std::fs::File::open(&update)?)?.len(), 2);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
