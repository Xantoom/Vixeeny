// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask dist-mac [--out <dir>]`: `Vixeeny.app`, its ad-hoc signature, the update archive
//! `Vixeeny-<version>-macos-<arch>.zip` and the disk image `Vixeeny-<version>-macos-<arch>.dmg`.
//!
//! Expects `cargo build --release -p vixeeny-daemon -p vixeeny-app` to have run. The bundle is
//! an agent application (`LSUIElement`): no Dock icon, the menu-bar icon is the interface.
//! The daemon is the bundle's main executable and starts `vixeeny-app` from the same folder.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Hardened-runtime exceptions: the microphone (recording) is the only one Vixeeny needs.
const ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.device.audio-input</key>
	<true/>
</dict>
</plist>
"#;

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>com.vixeeny.Vixeeny</string>
	<key>CFBundleName</key>
	<string>Vixeeny</string>
	<key>CFBundleDisplayName</key>
	<string>Vixeeny</string>
	<key>CFBundleIconFile</key>
	<string>Vixeeny</string>
	<key>CFBundleExecutable</key>
	<string>vixeeny-daemon</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{VERSION}</string>
	<key>CFBundleVersion</key>
	<string>{VERSION}</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSScreenCaptureUsageDescription</key>
	<string>Vixeeny captures the screen to take screenshots and record videos.</string>
	<key>NSMicrophoneUsageDescription</key>
	<string>Vixeeny records the microphone when you ask for it.</string>
</dict>
</plist>
"#
    )
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status().with_context(|| format!("{command:?}"))?;
    if !status.success() {
        bail!("{command:?} failed with {status}");
    }
    Ok(())
}

pub fn dist(args: &[String]) -> Result<()> {
    let out = match args {
        [flag, dir] if flag == "--out" => PathBuf::from(dir),
        [] => PathBuf::from("dist"),
        _ => bail!("usage: cargo xtask dist-mac [--out <dir>]"),
    };
    let root = root();
    let release = root.join("target/release");
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let base = format!("Vixeeny-{VERSION}-macos-{arch}");

    let stage = out.join("stage");
    let _ = std::fs::remove_dir_all(&stage);
    let app = stage.join("Vixeeny.app");
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    for program in ["vixeeny-daemon", "vixeeny-app"] {
        std::fs::copy(release.join(program), macos.join(program))
            .with_context(|| format!("{program} (run `cargo build --release` first)"))?;
    }
    // The updater joins the bundle once it exists for macOS.
    let updater = release.join("vixeeny-updater");
    if updater.exists() {
        std::fs::copy(updater, macos.join("vixeeny-updater"))?;
    }
    std::fs::write(app.join("Contents/Info.plist"), info_plist())?;
    let resources = app.join("Contents/Resources");
    std::fs::create_dir_all(&resources)?;
    std::fs::copy(root.join("LICENSE"), resources.join("LICENSE"))?;
    std::fs::copy(
        root.join("packaging/icons/vixeeny.icns"),
        resources.join("Vixeeny.icns"),
    )?;

    // Without `VIXEENY_SIGN_IDENTITY`: an ad hoc signature, which gives the bundle a stable
    // identity for the permission prompts. With it (a "Developer ID Application: …" identity in
    // the keychain): hardened runtime + secure timestamp, as notarization requires.
    let identity = std::env::var("VIXEENY_SIGN_IDENTITY")
        .ok()
        .filter(|v| !v.is_empty());
    match &identity {
        Some(identity) => {
            let entitlements = out.join("entitlements.plist");
            std::fs::write(&entitlements, ENTITLEMENTS)?;
            // Inside out: the executables first, then the bundle.
            for program in ["vixeeny-daemon", "vixeeny-app", "vixeeny-updater"] {
                let path = macos.join(program);
                if path.exists() {
                    run(Command::new("codesign")
                        .args([
                            "--force",
                            "--options",
                            "runtime",
                            "--timestamp",
                            "--entitlements",
                        ])
                        .arg(&entitlements)
                        .args(["--sign", identity])
                        .arg(&path))?;
                }
            }
            run(Command::new("codesign")
                .args([
                    "--force",
                    "--options",
                    "runtime",
                    "--timestamp",
                    "--entitlements",
                ])
                .arg(&entitlements)
                .args(["--sign", identity])
                .arg(&app))?;
            let _ = std::fs::remove_file(&entitlements);
        }
        None => run(Command::new("codesign")
            .args(["--force", "--deep", "--sign", "-"])
            .arg(&app))?,
    }

    let zip = out.join(format!("{base}.zip"));
    let _ = std::fs::remove_file(&zip);
    run(Command::new("ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(&app)
        .arg(&zip))?;

    let dmg_dir = out.join("dmg");
    let _ = std::fs::remove_dir_all(&dmg_dir);
    std::fs::create_dir_all(&dmg_dir)?;
    run(Command::new("cp").arg("-R").arg(&app).arg(&dmg_dir))?;
    std::os::unix::fs::symlink("/Applications", dmg_dir.join("Applications"))?;
    let dmg = out.join(format!("{base}.dmg"));
    let _ = std::fs::remove_file(&dmg);
    run(Command::new("hdiutil")
        .args([
            "create",
            "-volname",
            "Vixeeny",
            "-fs",
            "HFS+",
            "-format",
            "UDZO",
            "-srcfolder",
        ])
        .arg(&dmg_dir)
        .arg(&dmg))?;
    let _ = std::fs::remove_dir_all(&dmg_dir);
    if let Some(identity) = &identity {
        run(Command::new("codesign")
            .args(["--force", "--timestamp", "--sign", identity])
            .arg(&dmg))?;
    }
    println!("{}\n{}", zip.display(), dmg.display());
    Ok(())
}
