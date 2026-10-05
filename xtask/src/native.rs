// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask build-native`: download the pinned prebuilt FFmpeg, then fetch the image
//! libraries of `native/versions.toml` by exact commit and build them statically with MSVC into
//! `native/build/work/prefix`. Run it from a Visual Studio developer prompt.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Lib {
    url: String,
    commit: String,
    /// Informational upstream tag; re-created locally so `git describe` works (x265 needs it).
    tag: String,
}

/// The prebuilt FFmpeg (maintainer decision 2026-10-01): it bundles the video encoders.
#[derive(Deserialize)]
struct Prebuilt {
    url: String,
    sha256: String,
}

/// Bump a library's revision to force its rebuild when its recipe changes (stamps embed it).
fn recipe_rev(name: &str) -> &'static str {
    match name {
        "libjxl" => "3",
        "jpegli" => "5",
        _ => "2",
    }
}

/// Build order matters: libavif links against SVT-AV1 and dav1d.
const ORDER: &[&str] = &["svt-av1", "dav1d", "libwebp", "libjxl", "jpegli", "libavif"];

struct Ctx {
    work: PathBuf,
    prefix: PathBuf,
    jobs: String,
}

pub fn build(only: &[String]) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()?;
    // Drop the `\\?\` verbatim prefix: cygpath and the build tools do not understand it.
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"));
    let mut table: toml::Table =
        toml::from_str(&std::fs::read_to_string(root.join("native/versions.toml"))?)
            .context("parsing native/versions.toml")?;
    let prebuilt: Prebuilt = table
        .remove("ffmpeg")
        .context("ffmpeg missing from versions.toml")?
        .try_into()?;
    let libs: BTreeMap<String, Lib> = table.try_into()?;
    let work = root.join("native/build/work");
    let prefix = work.join("prefix");
    std::fs::create_dir_all(&prefix)?;
    let jobs = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .to_string();
    let ctx = Ctx { work, prefix, jobs };

    if only.is_empty() || only.iter().any(|o| o == "ffmpeg") {
        ffmpeg_prebuilt(&ctx, &prebuilt)?;
    }
    for name in ORDER {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        let lib = libs
            .get(*name)
            .with_context(|| format!("{name} missing from versions.toml"))?;
        let stamp = ctx.work.join(format!("{name}.stamp"));
        let stamp_value = format!("{}+{}", lib.commit, recipe_rev(name));
        if std::fs::read_to_string(&stamp).is_ok_and(|s| s == stamp_value) {
            println!("== {name}: up to date");
            continue;
        }
        println!("== {name}: fetching {}", lib.commit);
        let src = fetch(&ctx, name, lib)?;
        println!("== {name}: building");
        normalize_msvc_libs(&ctx)?;
        recipe(&ctx, name, &src)?;
        normalize_msvc_libs(&ctx)?;
        std::fs::write(&stamp, &stamp_value)?;
    }
    // Also for cached prefixes built before a normalisation rule existed.
    normalize_msvc_libs(&ctx)
}

/// Downloads the pinned prebuilt FFmpeg (shared, GPL) into `work/ffmpeg-prebuilt`, verifying
/// its SHA-256. `FFMPEG_DIR` must point there for `ffmpeg-sys-next`, and `bin` must be on PATH.
fn ffmpeg_prebuilt(ctx: &Ctx, pre: &Prebuilt) -> Result<()> {
    let dest = ctx.work.join("ffmpeg-prebuilt");
    let stamp = dest.join(".sha256");
    if std::fs::read_to_string(&stamp).is_ok_and(|s| s == pre.sha256) {
        println!("== ffmpeg (prebuilt): up to date");
        return msvc_import_libs(&dest);
    }
    println!("== ffmpeg (prebuilt): downloading {}", pre.url);
    let zip_path = ctx.work.join("ffmpeg-prebuilt.zip");
    run(
        &ctx.work,
        "curl",
        &[
            "-fsSL",
            "--retry",
            "5",
            "-o",
            "ffmpeg-prebuilt.zip",
            &pre.url,
        ],
        &[],
    )?;
    let digest = Sha256::digest(std::fs::read(&zip_path)?);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != pre.sha256 {
        bail!(
            "SHA-256 mismatch for {}: got {hex}, expected {}",
            pre.url,
            pre.sha256
        );
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest)?;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // Drop the single top-level directory of the archive.
        let Some(rel) = entry
            .enclosed_name()
            .and_then(|n| n.components().skip(1).collect::<PathBuf>().into())
        else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut std::fs::File::create(&out)?)?;
        }
    }
    std::fs::write(&stamp, &pre.sha256)?;
    msvc_import_libs(&dest)
}

/// Rewrites the import libraries `lib/<name>.lib` from the `.def` files, in the MSVC format.
/// The build's own are MinGW import libraries, which the linker cannot delay-load: with these,
/// `vixeeny-app` loads FFmpeg only when a video needs it (see its build script).
fn msvc_import_libs(ffmpeg: &Path) -> Result<()> {
    let lib = ffmpeg.join("lib");
    // `lib.exe` from the MSVC environment, else LLVM's (cross builds).
    let tool = if Command::new("lib").arg("/?").output().is_ok() {
        "lib"
    } else {
        "llvm-lib"
    };
    for entry in std::fs::read_dir(&lib)?.flatten() {
        let def = entry.file_name().to_string_lossy().into_owned();
        // `avcodec-63.def` describes `avcodec-63.dll`; the library is `avcodec.lib`.
        let Some(stem) = def.strip_suffix(".def") else {
            continue;
        };
        let Some((name, _)) = stem.rsplit_once('-') else {
            continue;
        };
        // The `.def` files only list the exports: the DLL's name is added for the library.
        let named = lib.join(format!("{stem}.def.msvc"));
        let exports = std::fs::read_to_string(entry.path())?;
        std::fs::write(&named, format!("LIBRARY \"{stem}.dll\"\n{exports}"))?;
        let result = run(
            &lib,
            tool,
            &[
                &format!("/def:{stem}.def.msvc"),
                "/machine:x64",
                &format!("/out:{name}.lib"),
                "/nologo",
            ],
            &[],
        );
        let _ = std::fs::remove_file(&named);
        result?;
    }
    println!("== ffmpeg: MSVC import libraries written ({tool})");
    Ok(())
}

fn run(dir: &Path, prog: &str, args: &[&str], env: &[(&str, String)]) -> Result<()> {
    let status = Command::new(prog)
        .args(args)
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (*k, v)))
        .status()
        .with_context(|| format!("spawning {prog}"))?;
    if !status.success() {
        bail!(
            "`{prog} {}` failed in {}: {status}",
            args.join(" "),
            dir.display()
        );
    }
    Ok(())
}

/// `cl` links `-lfoo` as `foo.lib`, so make every `libfoo.{a,lib}` available under
/// that name too, and drop Unix-only libraries from the `.pc` files.
fn normalize_msvc_libs(ctx: &Ctx) -> Result<()> {
    let dir = ctx.prefix.join("lib");
    if !dir.exists() {
        return Ok(());
    }
    // `.pc` files written for Unix name libraries MSVC does not have (`m.lib`, `pthread.lib`).
    if let Ok(pcs) = std::fs::read_dir(dir.join("pkgconfig")) {
        for pc in pcs {
            let path = pc?.path();
            let text = std::fs::read_to_string(&path)?;
            let cleaned: Vec<String> = text
                .lines()
                .map(|line| {
                    line.split(' ')
                        .filter(|t| {
                            !matches!(*t, "-lm" | "-lpthread" | "-ldl" | "-lstdc++" | "-UEB_DLL")
                        })
                        // libjxl.pc names `jxl-static`; the library is installed as `jxl.lib`.
                        .map(|t| if t == "-ljxl-static" { "-ljxl" } else { t })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect();
            std::fs::write(&path, cleaned.join("\n") + "\n")?;
        }
    }
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        let Some(file) = path.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        let Some(stem) = file
            .strip_prefix("lib")
            .and_then(|f| f.strip_suffix(".a").or_else(|| f.strip_suffix(".lib")))
        else {
            continue;
        };
        // Always overwrite: a rebuilt library must replace its stale `<name>.lib` copy.
        if file != format!("{stem}.lib") {
            std::fs::copy(&path, dir.join(format!("{stem}.lib")))?;
        }
    }
    Ok(())
}

/// Shallow-fetches exactly `lib.commit` (with submodules) into `work/<name>`.
fn fetch(ctx: &Ctx, name: &str, lib: &Lib) -> Result<PathBuf> {
    let src = ctx.work.join(name);
    if !src.join(".git").exists() {
        std::fs::create_dir_all(&src)?;
        run(&src, "git", &["init", "-q"], &[])?;
        run(&src, "git", &["remote", "add", "origin", &lib.url], &[])?;
    }
    run(
        &src,
        "git",
        &["fetch", "-q", "--depth", "1", "origin", &lib.commit],
        &[],
    )?;
    run(
        &src,
        "git",
        &["checkout", "-q", "--force", &lib.commit],
        &[],
    )?;
    if !lib.tag.contains(' ') {
        run(&src, "git", &["tag", "-f", &lib.tag, &lib.commit], &[])?;
    }
    run(
        &src,
        "git",
        &[
            "submodule",
            "update",
            "-q",
            "--init",
            "--recursive",
            "--depth",
            "1",
        ],
        &[],
    )?;
    Ok(src)
}

fn cmake(ctx: &Ctx, src: &Path, sub: &str, extra: &[&str]) -> Result<()> {
    let build = src.join("vx-build");
    let prefix = format!("-DCMAKE_INSTALL_PREFIX={}", ctx.prefix.display());
    let mut args = vec![
        "-S",
        sub,
        "-B",
        "vx-build",
        "-G",
        "Ninja",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        "-DBUILD_SHARED_LIBS=OFF",
        &prefix,
        "-DCMAKE_C_COMPILER=cl",
        "-DCMAKE_CXX_COMPILER=cl",
    ];
    args.extend_from_slice(extra);
    run(src, "cmake", &args, &[])?;
    let build = build.to_string_lossy().into_owned();
    run(src, "cmake", &["--build", &build, "-j", &ctx.jobs], &[])?;
    run(src, "cmake", &["--install", &build], &[])
}

fn recipe(ctx: &Ctx, name: &str, src: &Path) -> Result<()> {
    let prefix = ctx.prefix.display().to_string();
    match name {
        "svt-av1" => cmake(
            ctx,
            src,
            ".",
            &[
                "-DBUILD_APPS=OFF",
                "-DBUILD_DEC=OFF",
                "-DBUILD_TESTING=OFF",
                "-DSVT_AV1_LTO=OFF",
            ],
        ),
        "dav1d" => {
            run(
                src,
                "meson",
                &[
                    "setup",
                    "vx-build",
                    "--prefix",
                    &prefix,
                    "--libdir",
                    "lib",
                    "--buildtype=release",
                    "-Ddefault_library=static",
                    "-Db_vscrt=md",
                    "-Denable_tools=false",
                    "-Denable_tests=false",
                ],
                &[],
            )?;
            run(src, "ninja", &["-C", "vx-build", "install"], &[])
        }
        "libwebp" => cmake(
            ctx,
            src,
            ".",
            &[
                "-DWEBP_BUILD_ANIM_UTILS=OFF",
                "-DWEBP_BUILD_CWEBP=OFF",
                "-DWEBP_BUILD_DWEBP=OFF",
                "-DWEBP_BUILD_GIF2WEBP=OFF",
                "-DWEBP_BUILD_IMG2WEBP=OFF",
                "-DWEBP_BUILD_VWEBP=OFF",
                "-DWEBP_BUILD_WEBPINFO=OFF",
                "-DWEBP_BUILD_WEBPMUX=OFF",
                "-DWEBP_BUILD_EXTRAS=OFF",
            ],
        ),
        "libjxl" => cmake(
            ctx,
            src,
            ".",
            &[
                "-DJPEGXL_ENABLE_TOOLS=OFF",
                "-DJPEGXL_ENABLE_BENCHMARK=OFF",
                "-DJPEGXL_ENABLE_EXAMPLES=OFF",
                "-DJPEGXL_ENABLE_MANPAGES=OFF",
                "-DJPEGXL_ENABLE_DOXYGEN=OFF",
                "-DJPEGXL_ENABLE_JNI=OFF",
                "-DJPEGXL_ENABLE_SJPEG=OFF",
                "-DJPEGXL_ENABLE_OPENEXR=OFF",
                "-DJPEGXL_ENABLE_PLUGINS=OFF",
                "-DJPEGXL_ENABLE_VIEWERS=OFF",
                "-DJPEGXL_STATIC=ON",
                // JPEGXL_STATIC would default to the static CRT (/MT); everything else here and
                // the Rust side use /MD.
                "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL",
                "-DBUILD_TESTING=OFF",
            ],
        ),
        "jpegli" => {
            cmake(
                ctx,
                src,
                ".",
                &[
                    "-DJPEGXL_ENABLE_TOOLS=OFF",
                    "-DJPEGXL_ENABLE_BENCHMARK=OFF",
                    "-DJPEGXL_ENABLE_EXAMPLES=OFF",
                    "-DJPEGXL_ENABLE_MANPAGES=OFF",
                    "-DJPEGXL_ENABLE_DOXYGEN=OFF",
                    "-DJPEGXL_ENABLE_JNI=OFF",
                    "-DJPEGXL_ENABLE_SJPEG=OFF",
                    "-DJPEGXL_ENABLE_OPENEXR=OFF",
                    "-DJPEGXL_ENABLE_PLUGINS=OFF",
                    "-DJPEGXL_ENABLE_VIEWERS=OFF",
                    "-DJPEGXL_STATIC=ON",
                    // JPEGXL_STATIC would default to the static CRT (/MT); everything else here and
                    // the Rust side use /MD.
                    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL",
                    "-DBUILD_TESTING=OFF",
                ],
            )?;
            install_jpegli(ctx, src)
        }
        "libavif" => cmake(
            ctx,
            src,
            ".",
            &[
                "-DAVIF_CODEC_SVT=SYSTEM",
                "-DAVIF_CODEC_DAV1D=SYSTEM",
                "-DAVIF_LIBYUV=OFF",
                "-DAVIF_BUILD_APPS=OFF",
                "-DAVIF_BUILD_TESTS=OFF",
                &format!("-DCMAKE_PREFIX_PATH={prefix}"),
            ],
        ),
        other => bail!("no recipe for {other}"),
    }
}

/// jpegli's own install step leaves out the core library (only its helpers are installed), so
/// copy `jpegli-static` to `jpegli.lib` and describe it for pkg-config. Its C API
/// (`jpegli_*`) is declared by `vixeeny-image`.
fn install_jpegli(ctx: &Ctx, src: &Path) -> Result<()> {
    let built = src.join("vx-build/lib");
    let (from, to) = ("jpegli-static.lib", "jpegli.lib");
    std::fs::copy(built.join(from), ctx.prefix.join("lib").join(to))
        .with_context(|| format!("copying {from}"))?;
    let pc = format!(
        "prefix={p}\nlibdir=${{prefix}}/lib\nincludedir=${{prefix}}/include\n\n\
         Name: libjpegli\nDescription: jpegli, libjpeg-compatible JPEG codec\nVersion: 0.12.0\n\
         Requires: libhwy\nLibs: -L${{libdir}} -ljpegli\nCflags: -I${{includedir}}/jpegli\n",
        p = ctx.prefix.display()
    );
    let pc_dir = ctx.prefix.join("lib/pkgconfig");
    std::fs::create_dir_all(&pc_dir)?;
    std::fs::write(pc_dir.join("libjpegli.pc"), pc)?;
    Ok(())
}
