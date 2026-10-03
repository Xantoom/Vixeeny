// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask dist-linux [--out <dir>]`: the update archive `Vixeeny-<version>-linux-x64.zip`
//! (the programs and the desktop files side by side, what `vixeeny-updater` unpacks over a
//! portable install), the Debian package `vixeeny_<version>_amd64.deb` and the AppDir that
//! `appimagetool` turns into the AppImage (done by the release workflow).
//!
//! Expects `cargo build --release -p vixeeny-daemon -p vixeeny-app -p vixeeny-updater`.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const ID: &str = "io.github.Xantoom.Vixeeny";
const PROGRAMS: [&str; 3] = ["vixeeny-daemon", "vixeeny-app", "vixeeny-updater"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn packaging() -> PathBuf {
    root().join("packaging/linux")
}

fn put(from: &Path, to: &Path, mode: u32) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).with_context(|| from.display().to_string())?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn write(to: &Path, text: &str, mode: u32) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(to, text)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// The programs and licence files shared by the three outputs, under `bin` and `doc`.
fn install_programs(bin: &Path, doc: &Path) -> Result<()> {
    let release = root().join("target/release");
    for program in PROGRAMS {
        put(&release.join(program), &bin.join(program), 0o755)
            .with_context(|| format!("{program} is missing: run `cargo build --release` first"))?;
    }
    put(&root().join("LICENSE"), &doc.join("LICENSE"), 0o644)?;
    write(
        &doc.join("THIRD-PARTY-LICENSES.txt"),
        &crate::dist::third_party_licenses()?,
        0o644,
    )
}

fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            out.extend(files_under(&path)?);
        } else {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn write_zip(stage: &Path, out: &Path) -> Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(out)?);
    for path in files_under(stage)? {
        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(mode);
        let name = path
            .strip_prefix(stage)?
            .to_string_lossy()
            .replace('\\', "/");
        zip.start_file(name, options)?;
        zip.write_all(&std::fs::read(&path)?)?;
    }
    zip.finish()?;
    Ok(())
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status().with_context(|| format!("{command:?}"))?;
    if !status.success() {
        bail!("{command:?} failed with {status}");
    }
    Ok(())
}

fn control(installed_kib: u64) -> String {
    format!(
        "Package: vixeeny\nVersion: {VERSION}\nSection: graphics\nPriority: optional\nArchitecture: amd64\n\
         Installed-Size: {installed_kib}\nMaintainer: Xantoom <xantoom@gmail.com>\n\
         Depends: libc6 (>= 2.39), libfontconfig1, libpipewire-0.3-0, libxkbcommon0\n\
         Recommends: tesseract-ocr, wl-clipboard, xclip, libnotify-bin\n\
         Homepage: https://github.com/Xantoom/Vixeeny\n\
         Description: Screenshots, screen recording and text recognition\n \
         Vixeeny lives in the system tray and answers to global shortcuts: it captures a\n \
         region, a window or the screen, edits the capture, reads its text and records the\n \
         screen with hardware encoders.\n"
    )
}

pub fn dist(args: &[String]) -> Result<()> {
    let out = match args {
        [flag, dir] if flag == "--out" => PathBuf::from(dir),
        [] => PathBuf::from("dist"),
        _ => bail!("usage: cargo xtask dist-linux [--out <dir>]"),
    };
    std::fs::create_dir_all(&out)?;
    let pk = packaging();

    // Update archive: everything flat, like the Windows one.
    let stage = out.join("stage");
    let _ = std::fs::remove_dir_all(&stage);
    install_programs(&stage, &stage)?;
    for name in [
        format!("{ID}.desktop"),
        format!("{ID}.svg"),
        format!("{ID}.metainfo.xml"),
    ] {
        put(&pk.join(&name), &stage.join(&name), 0o644)?;
    }
    put(
        &root().join("packaging/icons/vixeeny-256.png"),
        &stage.join(format!("{ID}.png")),
        0o644,
    )?;
    let zip = out.join(format!("Vixeeny-{VERSION}-linux-x64.zip"));
    write_zip(&stage, &zip)?;

    // Debian package.
    let deb_root = out.join("deb");
    let _ = std::fs::remove_dir_all(&deb_root);
    install_programs(
        &deb_root.join("usr/bin"),
        &deb_root.join("usr/share/doc/vixeeny"),
    )?;
    // The licence files are documentation, not programs.
    let bin = deb_root.join("usr/bin");
    for stray in ["LICENSE", "THIRD-PARTY-LICENSES.txt"] {
        let _ = std::fs::remove_file(bin.join(stray));
    }
    put(
        &pk.join(format!("{ID}.desktop")),
        &deb_root.join(format!("usr/share/applications/{ID}.desktop")),
        0o644,
    )?;
    put(
        &pk.join(format!("{ID}.metainfo.xml")),
        &deb_root.join(format!("usr/share/metainfo/{ID}.metainfo.xml")),
        0o644,
    )?;
    put(
        &pk.join(format!("{ID}.svg")),
        &deb_root.join(format!("usr/share/icons/hicolor/scalable/apps/{ID}.svg")),
        0o644,
    )?;
    for size in [16, 24, 32, 48, 64, 128, 256, 512] {
        put(
            &root().join(format!("packaging/icons/vixeeny-{size}.png")),
            &deb_root.join(format!(
                "usr/share/icons/hicolor/{size}x{size}/apps/{ID}.png"
            )),
            0o644,
        )?;
    }
    let kib: u64 = files_under(&deb_root)?
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum::<u64>()
        / 1024;
    write(&deb_root.join("DEBIAN/control"), &control(kib), 0o644)?;
    let deb = out.join(format!("vixeeny_{VERSION}_amd64.deb"));
    let _ = std::fs::remove_file(&deb);
    run(Command::new("dpkg-deb")
        .args(["--build", "--root-owner-group", "-Zxz"])
        .arg(&deb_root)
        .arg(&deb))?;

    // AppDir for appimagetool.
    let appdir = out.join("Vixeeny.AppDir");
    let _ = std::fs::remove_dir_all(&appdir);
    install_programs(
        &appdir.join("usr/bin"),
        &appdir.join("usr/share/doc/vixeeny"),
    )?;
    for stray in ["LICENSE", "THIRD-PARTY-LICENSES.txt"] {
        let _ = std::fs::remove_file(appdir.join("usr/bin").join(stray));
    }
    put(
        &pk.join(format!("{ID}.desktop")),
        &appdir.join(format!("{ID}.desktop")),
        0o644,
    )?;
    put(
        &pk.join(format!("{ID}.svg")),
        &appdir.join(format!("{ID}.svg")),
        0o644,
    )?;
    put(
        &pk.join(format!("{ID}.metainfo.xml")),
        &appdir.join(format!("usr/share/metainfo/{ID}.metainfo.xml")),
        0o644,
    )?;
    // The AppImage icon: `<id>.png` next to the desktop file and `.DirIcon`.
    for name in [format!("{ID}.png"), ".DirIcon".to_owned()] {
        put(
            &root().join("packaging/icons/vixeeny-256.png"),
            &appdir.join(name),
            0o644,
        )?;
    }
    write(
        &appdir.join("AppRun"),
        "#!/bin/sh\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexec \"$HERE/usr/bin/vixeeny-daemon\" \"$@\"\n",
        0o755,
    )?;
    let _ = std::fs::remove_dir_all(&deb_root);
    println!("{}\n{}\n{}", zip.display(), deb.display(), appdir.display());
    Ok(())
}
