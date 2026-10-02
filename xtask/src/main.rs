// SPDX-License-Identifier: GPL-3.0-or-later
//! Build and maintenance tasks: `cargo xtask <task>`.

mod bench;
mod native;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("build-native") => native::build(&args.collect::<Vec<_>>()),
        Some("bench-idle") => bench::run(&args.collect::<Vec<_>>()),
        Some("verify-registry") => verify_registry(),
        Some("dist" | "bench-latency" | "test-hw") => {
            bail!("task not implemented yet (see VIXEENY_PLAN.md section 11)")
        }
        _ => bail!(
            "usage: cargo xtask <build-native [lib…]|dist|verify-registry|bench-idle|bench-latency|test-hw>"
        ),
    }
}

/// `cargo xtask verify-registry`: compares `registry.toml` with the encoders of the linked
/// FFmpeg (the native build of `cargo xtask build-native`).
fn verify_registry() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let work = root.join("native/build/work");
    let mut cmd = std::process::Command::new("cargo");
    cmd.current_dir(&root).args([
        "test",
        "-p",
        "vixeeny-encode",
        "--features",
        "ffmpeg-next",
        "--test",
        "verify_registry",
        "--",
        "--nocapture",
    ]);
    if cfg!(windows) {
        let ffmpeg = work.join("ffmpeg-prebuilt");
        let bin = ffmpeg.join("bin");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path)))?;
        cmd.env("FFMPEG_DIR", &ffmpeg).env("PATH", joined);
    } else {
        cmd.env("PKG_CONFIG_PATH", work.join("prefix/lib/pkgconfig"));
    }
    let status = cmd.status()?;
    if !status.success() {
        bail!("the registry does not match FFmpeg (see the mismatches above)");
    }
    Ok(())
}
