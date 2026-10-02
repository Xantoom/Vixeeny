// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask build-native`: fetch the pinned native libraries of `native/versions.toml`
//! by exact commit and build them statically into `native/build/work/prefix`.

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

/// Prebuilt FFmpeg used on Windows instead of building it (maintainer decision 2026-10-01).
#[derive(Deserialize)]
struct Prebuilt {
    url: String,
    sha256: String,
}

/// Libraries not built on Windows: FFmpeg ships prebuilt, bundling these encoders.
const WINDOWS_PREBUILT: &[&str] = &["x264", "x265", "libvpx", "opus", "ffmpeg"];

/// Bump a library's revision to force its rebuild when its recipe changes (stamps embed it).
fn recipe_rev(name: &str) -> &'static str {
    match name {
        "libvpx" => "6",
        "x265" => "3",
        "libjxl" => "3",
        "jpegli" => "5",
        _ => "2",
    }
}

/// Build order matters: FFmpeg links against everything before it.
const ORDER: &[&str] = &[
    "x264", "x265", "libvpx", "svt-av1", "dav1d", "opus", "ffmpeg", "libwebp", "libjxl", "jpegli",
    "libavif",
];

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
        .remove("ffmpeg-windows-prebuilt")
        .context("ffmpeg-windows-prebuilt missing from versions.toml")?
        .try_into()?;
    let libs: BTreeMap<String, Lib> = table.try_into()?;
    let work = root.join("native/build/work");
    let prefix = work.join("prefix");
    std::fs::create_dir_all(&prefix)?;
    let jobs = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .to_string();
    let ctx = Ctx { work, prefix, jobs };

    if cfg!(windows) && (only.is_empty() || only.iter().any(|o| o == "ffmpeg")) {
        ffmpeg_prebuilt(&ctx, &prebuilt)?;
    }
    for name in ORDER {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        if cfg!(windows) && WINDOWS_PREBUILT.contains(name) {
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
        if cfg!(windows) {
            normalize_msvc_libs(&ctx)?;
        }
        recipe(&ctx, name, &src)?;
        if cfg!(windows) {
            normalize_msvc_libs(&ctx)?;
        }
        std::fs::write(&stamp, &stamp_value)?;
    }
    if cfg!(windows) {
        // Also for cached prefixes built before a normalisation rule existed.
        normalize_msvc_libs(&ctx)?;
    }
    Ok(())
}

/// Downloads the pinned prebuilt FFmpeg (shared, GPL) into `work/ffmpeg-prebuilt`, verifying
/// its SHA-256. `FFMPEG_DIR` must point there for `ffmpeg-sys-next`, and `bin` must be on PATH.
fn ffmpeg_prebuilt(ctx: &Ctx, pre: &Prebuilt) -> Result<()> {
    let dest = ctx.work.join("ffmpeg-prebuilt");
    let stamp = dest.join(".sha256");
    if std::fs::read_to_string(&stamp).is_ok_and(|s| s == pre.sha256) {
        println!("== ffmpeg (prebuilt): up to date");
        return Ok(());
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

/// Runs `script` in `dir` through bash. Unix only (autotools-style libraries).
fn sh(dir: &Path, script: &str, env: &[(&str, String)]) -> Result<()> {
    run(
        Path::new("."),
        "bash",
        &["-c", &format!("cd '{}' && {script}", dir.display())],
        env,
    )
}

/// Windows: `cl` links `-lfoo` as `foo.lib`, so make every `libfoo.{a,lib}` available under
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
    ];
    if cfg!(windows) {
        args.extend(["-DCMAKE_C_COMPILER=cl", "-DCMAKE_CXX_COMPILER=cl"]);
    }
    args.extend_from_slice(extra);
    run(src, "cmake", &args, &[])?;
    let build = build.to_string_lossy().into_owned();
    run(src, "cmake", &["--build", &build, "-j", &ctx.jobs], &[])?;
    run(src, "cmake", &["--install", &build], &[])
}

fn recipe(ctx: &Ctx, name: &str, src: &Path) -> Result<()> {
    let prefix = ctx.prefix.display().to_string();
    let pkg_env = [(
        "PKG_CONFIG_PATH",
        format!("{prefix}/lib/pkgconfig:{prefix}/lib64/pkgconfig"),
    )];
    match name {
        "x264" => sh(
            src,
            &format!(
                "./configure --prefix='{prefix}' --enable-static --enable-pic --disable-cli \
                 --disable-opencl && make -j{j} && make install",
                j = ctx.jobs
            ),
            &[],
        ),
        "x265" => x265(ctx, src),
        "libvpx" => sh(
            src,
            &format!(
                "./configure --prefix='{prefix}' --enable-static --disable-shared --enable-pic \
                 --enable-vp9-highbitdepth --disable-examples --disable-tools --disable-docs \
                 --disable-unit-tests && make -j{j} && make install",
                j = ctx.jobs
            ),
            &[],
        ),
        // LTO off: GCC "slim" LTO objects have no symbols in the archive index, so lld (Rust's
        // linker) cannot resolve libSvtAv1Enc.a.
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
        "opus" => cmake(
            ctx,
            src,
            ".",
            &["-DOPUS_BUILD_PROGRAMS=OFF", "-DOPUS_BUILD_TESTING=OFF"],
        ),
        "ffmpeg" => sh(
            src,
            &format!(
                "./configure --enable-pic --prefix='{prefix}' --enable-gpl --enable-version3 \
                 --enable-static --disable-shared --disable-programs --disable-doc \
                 --disable-debug --pkg-config-flags=--static --enable-libx264 --enable-libx265 \
                 --enable-libvpx --enable-libsvtav1 --enable-libdav1d --enable-libopus \
                 && make -j{j} && make install",
                j = ctx.jobs
            ),
            &pkg_env,
        ),
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
        "libavif" => {
            let _ = pkg_env;
            cmake(
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
            )
        }
        other => bail!("no recipe for {other}"),
    }
}

/// jpegli's own install step leaves out the core library (only its helpers are installed), so
/// copy `jpegli-static` to `libjpegli.a` / `jpegli.lib` and describe it for pkg-config. Its C API
/// (`jpegli_*`) is declared by `vixeeny-image`.
fn install_jpegli(ctx: &Ctx, src: &Path) -> Result<()> {
    let built = src.join("vx-build/lib");
    let (from, to) = if cfg!(windows) {
        ("jpegli-static.lib", "jpegli.lib")
    } else {
        ("libjpegli-static.a", "libjpegli.a")
    };
    std::fs::copy(built.join(from), ctx.prefix.join("lib").join(to))
        .with_context(|| format!("copying {from}"))?;
    let cxx = if cfg!(target_os = "macos") {
        "-lc++"
    } else {
        "-lstdc++"
    };
    let pc = format!(
        "prefix={p}\nlibdir=${{prefix}}/lib\nincludedir=${{prefix}}/include\n\n\
         Name: libjpegli\nDescription: jpegli, libjpeg-compatible JPEG codec\nVersion: 0.12.0\n\
         Requires: libhwy\nLibs: -L${{libdir}} -ljpegli -lm {cxx}\nCflags: -I${{includedir}}/jpegli\n",
        p = ctx.prefix.display()
    );
    let pc_dir = ctx.prefix.join("lib/pkgconfig");
    std::fs::create_dir_all(&pc_dir)?;
    std::fs::write(pc_dir.join("libjpegli.pc"), pc)?;
    Ok(())
}

/// x265 multilib (8/10-bit in one static library; 12-bit deliberately not built).
/// Unix only: Windows uses the prebuilt FFmpeg.
fn x265(ctx: &Ctx, src: &Path) -> Result<()> {
    let prefix = format!("-DCMAKE_INSTALL_PREFIX={}", ctx.prefix.display());
    let common = [
        "-G",
        "Ninja",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        "-DENABLE_SHARED=OFF",
        "-DENABLE_CLI=OFF",
        "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
        // x265 4.2 json11 lacks <cstdint>, which recent GCC no longer pulls in transitively.
        "-DCMAKE_CXX_FLAGS=-include cstdint",
    ];
    let d10 = src.join("vx10");
    let d8 = src.join("vx8");

    std::fs::create_dir_all(&d10)?;
    let mut args = vec!["../source"];
    args.extend(common);
    args.extend([
        "-DHIGH_BIT_DEPTH=ON",
        "-DEXPORT_C_API=OFF",
        "-DENABLE_HDR10_PLUS=ON",
        "-DMAIN10=ON",
    ]);
    run(&d10, "cmake", &args, &[])?;
    run(&d10, "ninja", &["-j", &ctx.jobs], &[])?;

    std::fs::create_dir_all(&d8)?;
    std::fs::copy(d10.join("libx265.a"), d8.join("libx265_main10.a"))?;
    let mut args = vec!["../source"];
    args.extend(common);
    args.extend([
        "-DEXTRA_LIB=libx265_main10.a",
        "-DEXTRA_LINK_FLAGS=-L.",
        "-DLINKED_10BIT=ON",
        "-DENABLE_HDR10_PLUS=ON",
        &prefix,
    ]);
    run(&d8, "cmake", &args, &[])?;
    run(&d8, "ninja", &["-j", &ctx.jobs], &[])?;
    run(&d8, "ninja", &["install"], &[])?;

    // Merge the 8-bit and 10-bit archives into the installed libx265.a.
    let libdir = ctx.prefix.join("lib");
    std::fs::rename(d8.join("libx265.a"), d8.join("libx265_main.a"))?;
    if cfg!(target_os = "macos") {
        // Apple's `ar` has no MRI scripts.
        run(
            &d8,
            "libtool",
            &[
                "-static",
                "-o",
                "libx265.a",
                "libx265_main.a",
                "libx265_main10.a",
            ],
            &[],
        )?;
    } else {
        let script =
            "create libx265.a\naddlib libx265_main.a\naddlib libx265_main10.a\nsave\nend\n";
        std::fs::write(d8.join("merge.mri"), script)?;
        run(&d8, "sh", &["-c", "ar -M < merge.mri"], &[])?;
    }
    std::fs::copy(d8.join("libx265.a"), libdir.join("libx265.a"))?;

    // x265 only generates its .pc file for shared builds; FFmpeg's configure needs one.
    let p = ctx.prefix.display();
    let cxx = if cfg!(target_os = "macos") {
        "-lc++"
    } else {
        "-lstdc++"
    };
    let pc = format!(
        "prefix={p}\nlibdir={p}/lib\nincludedir={p}/include\n\nName: x265\n\
         Description: H.265/HEVC video encoder (8/10-bit)\nVersion: 4.2\n\
         Libs: -L${{libdir}} -lx265\nLibs.private: -lhdr10plus {cxx} -lm -ldl -lpthread\n\
         Cflags: -I${{includedir}}\n"
    );
    std::fs::create_dir_all(libdir.join("pkgconfig"))?;
    std::fs::write(libdir.join("pkgconfig/x265.pc"), pc)?;
    Ok(())
}
