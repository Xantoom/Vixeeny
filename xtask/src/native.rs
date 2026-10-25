// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask build-native`: unpack FFmpeg (`native/build/work/ffmpeg.zip`, made by `packaging/ffmpeg/build.sh`),
//! then fetch the image libraries of `native/versions.toml` by exact commit and build them statically with MSVC into
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

/// Bump a library's revision to force its rebuild when its recipe changes (stamps embed it).
fn recipe_rev(name: &str) -> &'static str {
    match name {
        "libjxl" => "3",
        "jpegli" => "5",
        _ => "2",
    }
}

/// The order they are built in.
const ORDER: &[&str] = &["libjxl", "jpegli", "libjpeg-turbo", "libultrahdr"];

struct Ctx {
    work: PathBuf,
    prefix: PathBuf,
    jobs: String,
}

pub fn build(only: &[String]) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize()?;
    // Drop the `\\?\` verbatim prefix: cygpath and the build tools do not understand it.
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"));
    let mut table: toml::Table = toml::from_str(&std::fs::read_to_string(root.join("native/versions.toml"))?)
        .context("parsing native/versions.toml")?;
    // FFmpeg is described there but built by packaging/ffmpeg/build.sh.
    table.remove("ffmpeg");
    let libs: BTreeMap<String, Lib> = table.try_into()?;
    let work = root.join("native/build/work");
    let prefix = work.join("prefix");
    std::fs::create_dir_all(&prefix)?;
    let jobs = std::thread::available_parallelism().map_or(2, std::num::NonZero::get).to_string();
    let ctx = Ctx { work, prefix, jobs };

    if only.is_empty() || only.iter().any(|o| o == "ffmpeg") {
        ffmpeg_prebuilt(&ctx)?;
    }
    for name in ORDER {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        let lib = libs.get(*name).with_context(|| format!("{name} missing from versions.toml"))?;
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

/// Downloads the pinned prebuilt FFmpeg (shared, GPL) into `work/ffmpeg-prebuilt`, verifying its SHA-256. `FFMPEG_DIR`
/// must point there for `ffmpeg-sys-next`, and `bin` must be on PATH.
fn ffmpeg_prebuilt(ctx: &Ctx) -> Result<()> {
    let dest = ctx.work.join("ffmpeg-prebuilt");
    let stamp = dest.join(".sha256");
    let zip_path = ctx.work.join("ffmpeg.zip");
    let Ok(bytes) = std::fs::read(&zip_path) else {
        bail!(
            "{} is missing: build it with `packaging/ffmpeg/build.sh {}` (Docker), or take the \
             `ffmpeg` artifact of a CI run (`gh run download -n ffmpeg -D native/build/work`)",
            zip_path.display(),
            zip_path.display()
        );
    };
    let hex: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if std::fs::read_to_string(&stamp).is_ok_and(|s| s == hex) {
        println!("== ffmpeg: up to date");
        return msvc_import_libs(&dest);
    }
    println!("== ffmpeg: unpacking {}", zip_path.display());
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest)?;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // Drop the single top-level directory of the archive.
        let Some(rel) = entry.enclosed_name().and_then(|n| n.components().skip(1).collect::<PathBuf>().into()) else {
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
    std::fs::write(&stamp, hex)?;
    msvc_import_libs(&dest)
}

/// Rewrites the import libraries `lib/<name>.lib` from the `.def` files, in the MSVC format. The build's own are MinGW
/// import libraries, which the linker cannot delay-load: with these, `vixeeny-app` loads FFmpeg only when a video needs
/// it (see its build script).
fn msvc_import_libs(ffmpeg: &Path) -> Result<()> {
    let lib = ffmpeg.join("lib");
    // `lib.exe` from the MSVC environment, else LLVM's (cross builds).
    let tool = if Command::new("lib").arg("/?").output().is_ok() { "lib" } else { "llvm-lib" };
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
            &[&format!("/def:{stem}.def.msvc"), "/machine:x64", &format!("/out:{name}.lib"), "/nologo"],
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
        bail!("`{prog} {}` failed in {}: {status}", args.join(" "), dir.display());
    }
    Ok(())
}

/// `cl` links `-lfoo` as `foo.lib`, so make every `libfoo.{a,lib}` available under that name too, and drop Unix-only
/// libraries from the `.pc` files.
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
                        .filter(|t| !matches!(*t, "-lm" | "-lpthread" | "-ldl" | "-lstdc++" | "-UEB_DLL"))
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
        let Some(stem) = file.strip_prefix("lib").and_then(|f| f.strip_suffix(".a").or_else(|| f.strip_suffix(".lib")))
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
    run(&src, "git", &["fetch", "-q", "--depth", "1", "origin", &lib.commit], &[])?;
    run(&src, "git", &["checkout", "-q", "--force", &lib.commit], &[])?;
    if !lib.tag.contains(' ') {
        run(&src, "git", &["tag", "-f", &lib.tag, &lib.commit], &[])?;
    }
    run(&src, "git", &["submodule", "update", "-q", "--init", "--recursive", "--depth", "1"], &[])?;
    Ok(src)
}

fn cmake(ctx: &Ctx, src: &Path, sub: &str, extra: &[&str]) -> Result<()> {
    configure_and_build(ctx, src, sub, extra)?;
    let build = src.join("vx-build").to_string_lossy().into_owned();
    run(src, "cmake", &["--install", &build], &[])
}

fn configure_and_build(ctx: &Ctx, src: &Path, sub: &str, extra: &[&str]) -> Result<()> {
    let build = src.join("vx-build");
    let prefix = format!("-DCMAKE_INSTALL_PREFIX={}", ctx.prefix.display());
    // A later `-DCMAKE_INSTALL_PREFIX` in `extra` wins.
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
    let toolchain = compilers()?;
    args.extend(toolchain.iter().map(String::as_str));
    args.extend_from_slice(extra);
    run(src, "cmake", &args, &[])?;
    let build = build.to_string_lossy().into_owned();
    run(src, "cmake", &["--build", &build, "-j", &ctx.jobs], &[])
}

/// MSVC on Windows. Elsewhere (WSL) the Windows libraries are cross-built with the clang-cl toolchain of cargo-xwin, so
/// that the Windows tests run from Linux link them too.
fn compilers() -> Result<Vec<String>> {
    if cfg!(windows) {
        return Ok(vec!["-DCMAKE_C_COMPILER=cl".into(), "-DCMAKE_CXX_COMPILER=cl".into()]);
    }
    let home = std::env::var("HOME").context("HOME")?;
    let toolchain = Path::new(&home).join(".cache/cargo-xwin/cmake/clang-cl/x86_64-pc-windows-msvc-toolchain.cmake");
    if !toolchain.exists() {
        bail!("{} is missing: run any `cargo xwin` build once to create it", toolchain.display());
    }
    Ok(vec![format!("-DCMAKE_TOOLCHAIN_FILE={}", toolchain.display())])
}

/// The options of libjxl, and of jpegli, which builds from the same tree.
const JPEGXL: &[&str] = &[
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
    // JPEGXL_STATIC would default to the static CRT (/MT); everything else here and the Rust side use /MD.
    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL",
    "-DBUILD_TESTING=OFF",
];

fn recipe(ctx: &Ctx, name: &str, src: &Path) -> Result<()> {
    match name {
        "libjxl" => cmake(ctx, src, ".", JPEGXL),
        "jpegli" => {
            cmake(ctx, src, ".", JPEGXL)?;
            install_jpegli(ctx, src)
        }
        // Only for libultrahdr, installed apart: its `jpeglib.h` must not shadow jpegli's.
        "libjpeg-turbo" => {
            let turbo = format!("-DCMAKE_INSTALL_PREFIX={}", turbo_prefix(ctx).display());
            configure_and_build(
                ctx,
                src,
                ".",
                &[
                    "-DENABLE_SHARED=OFF",
                    "-DWITH_CRT_DLL=ON",
                    "-DWITH_TURBOJPEG=OFF",
                    "-DWITH_TOOLS=OFF",
                    "-DWITH_TESTS=OFF",
                    &turbo,
                ],
            )?;
            let build = src.join("vx-build").to_string_lossy().into_owned();
            run(src, "cmake", &["--install", &build], &[])
        }
        "libultrahdr" => {
            // Forward slashes: libultrahdr's CMake re-reads these paths as strings, where a backslash starts an escape.
            let turbo = turbo_prefix(ctx).to_string_lossy().replace('\\', "/");
            let include = format!("-DJPEG_INCLUDE_DIR={turbo}/include");
            let library = format!("-DJPEG_LIBRARY={turbo}/lib/jpeg-static.lib");
            configure_and_build(
                ctx,
                src,
                ".",
                &[
                    &include,
                    &library,
                    "-DUHDR_ENABLE_HEIF=OFF",
                    "-DUHDR_BUILD_EXAMPLES=OFF",
                    "-DUHDR_ENABLE_INSTALL=OFF",
                    // A static build would otherwise force the static CRT (/MT); this switch keeps the runtime given
                    // below (/MD), like everything else here and the Rust side.
                    "-DBUILD_FOR_WINUI=ON",
                    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL",
                ],
            )?;
            install_ultrahdr(ctx, src)
        }
        other => bail!("no recipe for {other}"),
    }
}

/// jpegli's own install step leaves out the core library (only its helpers are installed), so copy `jpegli-static` to
/// `jpegli.lib` and describe it for pkg-config. Its C API (`jpegli_*`) is declared by `vixeeny-image`.
fn install_jpegli(ctx: &Ctx, src: &Path) -> Result<()> {
    let built = src.join("vx-build/lib");
    let (from, to) = ("jpegli-static.lib", "jpegli.lib");
    std::fs::copy(built.join(from), ctx.prefix.join("lib").join(to)).with_context(|| format!("copying {from}"))?;
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

fn turbo_prefix(ctx: &Ctx) -> PathBuf {
    ctx.work.join("turbo-prefix")
}

/// libultrahdr installs nothing when cross-built: copy its library, its libjpeg-turbo and its header, and describe them
/// for pkg-config.
fn install_ultrahdr(ctx: &Ctx, src: &Path) -> Result<()> {
    let lib = ctx.prefix.join("lib");
    for (from, to) in [
        (src.join("vx-build/uhdr-static.lib"), "uhdr.lib"),
        (turbo_prefix(ctx).join("lib/jpeg-static.lib"), "uhdr_jpeg.lib"),
    ] {
        std::fs::copy(&from, lib.join(to)).with_context(|| format!("copying {}", from.display()))?;
    }
    std::fs::copy(src.join("ultrahdr_api.h"), ctx.prefix.join("include/ultrahdr_api.h"))
        .context("copying ultrahdr_api.h")?;
    let pc = format!(
        "prefix={p}\nlibdir=${{prefix}}/lib\nincludedir=${{prefix}}/include\n\n\
         Name: libuhdr\nDescription: libultrahdr, JPEG with an HDR gain map\nVersion: 2.0.2\n\
         Libs: -L${{libdir}} -luhdr -luhdr_jpeg\nCflags: -I${{includedir}}\n",
        p = ctx.prefix.display()
    );
    std::fs::write(lib.join("pkgconfig/libuhdr.pc"), pc)?;
    Ok(())
}
