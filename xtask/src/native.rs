// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask build-native`: fetch the pinned native libraries of `native/versions.toml`
//! by exact commit and build them statically into `native/build/work/prefix`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Deserialize)]
struct Lib {
    url: String,
    commit: String,
    /// Informational upstream tag; re-created locally so `git describe` works (x265 needs it).
    tag: String,
}

/// Bump to force a rebuild of every library when a recipe changes (stamps embed it).
const RECIPE_REV: &str = "2";

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
    let libs: BTreeMap<String, Lib> =
        toml::from_str(&std::fs::read_to_string(root.join("native/versions.toml"))?)
            .context("parsing native/versions.toml")?;
    let work = root.join("native/build/work");
    let prefix = work.join("prefix");
    std::fs::create_dir_all(&prefix)?;
    let jobs = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .to_string();
    let ctx = Ctx { work, prefix, jobs };

    for name in ORDER {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        let lib = libs
            .get(*name)
            .with_context(|| format!("{name} missing from versions.toml"))?;
        let stamp = ctx.work.join(format!("{name}.stamp"));
        let stamp_value = format!("{}+{RECIPE_REV}", lib.commit);
        if std::fs::read_to_string(&stamp).is_ok_and(|s| s == stamp_value) {
            println!("== {name}: up to date");
            continue;
        }
        println!("== {name}: fetching {}", lib.commit);
        let src = fetch(&ctx, name, lib)?;
        println!("== {name}: building");
        recipe(&ctx, name, &src)?;
        if cfg!(windows) {
            normalize_msvc_libs(&ctx)?;
        }
        std::fs::write(&stamp, &stamp_value)?;
    }
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

/// Converts a path for use inside the MSYS2 shell (`C:\\x` -> `/c/x`); identity elsewhere.
fn unix(p: &Path) -> String {
    if !cfg!(windows) {
        return p.display().to_string();
    }
    let out = Command::new("cygpath").arg("-u").arg(p).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        _ => p.display().to_string().replace('\\', "/"),
    }
}

/// Runs `script` in `dir` through bash (MSYS2 on Windows, where MSVC's `cl` must already be on
/// PATH, e.g. via `vcvars64`).
fn sh(dir: &Path, script: &str, env: &[(&str, String)]) -> Result<()> {
    run(
        Path::new("."),
        // `bash` on Windows PATH can be WSL's launcher; MSYS2's `sh` is bash.
        if cfg!(windows) { "sh" } else { "bash" },
        &["-c", &format!("cd '{}' && {script}", unix(dir))],
        env,
    )
}

/// FFmpeg's MSVC configure links `-lfoo` as `foo.lib`; make every `libfoo.{a,lib}` available
/// under that name too.
fn normalize_msvc_libs(ctx: &Ctx) -> Result<()> {
    let dir = ctx.prefix.join("lib");
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
        let target = dir.join(format!("{stem}.lib"));
        if !target.exists() {
            std::fs::copy(&path, target)?;
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
    let up = unix(&ctx.prefix);
    let pkg_env = [(
        "PKG_CONFIG_PATH",
        format!("{up}/lib/pkgconfig:{up}/lib64/pkgconfig"),
    )];
    let win = cfg!(windows);
    match name {
        "x264" => {
            // Windows: /MD like the Rust MSVC target (cl defaults to /MT).
            let (cc, crt) = if win {
                ("CC=cl ", "--extra-cflags=-MD ")
            } else {
                ("", "")
            };
            sh(
                src,
                &format!(
                    "{cc}./configure {crt}--prefix='{up}' --enable-static --enable-pic --disable-cli \
                     --disable-opencl && make -j{j} && make install",
                    j = ctx.jobs
                ),
                &[],
            )
        }
        "x265" => x265(ctx, src),
        "libvpx" => {
            // On Windows: MinGW gcc (plain C + nasm, no C++ runtime) so the archive links with
            // MSVC; libvpx's own MSVC target needs yasm VS integration.
            let (path, target) = if win {
                ("PATH=/mingw64/bin:$PATH ", "--target=x86_64-win64-gcc ")
            } else {
                ("", "")
            };
            sh(
                src,
                &format!(
                    "{path}./configure {target}--prefix='{up}' --enable-static --disable-shared \
                     --enable-pic --enable-vp9-highbitdepth --disable-examples --disable-tools \
                     --disable-docs --disable-unit-tests && {path}make -j{j} && {path}make install",
                    j = ctx.jobs
                ),
                &[],
            )
        }
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
        "ffmpeg" => {
            let tc = if win {
                "--toolchain=msvc --target-os=win64 --arch=x86_64 --extra-cflags=-MD "
            } else {
                "--enable-pic "
            };
            sh(
                src,
                &format!(
                    "./configure {tc}--prefix='{up}' --enable-gpl --enable-version3 \
                     --enable-static --disable-shared --disable-programs --disable-doc \
                     --disable-debug --pkg-config-flags=--static --enable-libx264 \
                     --enable-libx265 --enable-libvpx --enable-libsvtav1 --enable-libdav1d \
                     --enable-libopus || {{ \
                     grep -a -A22 'check_func_headers EbSvtAv1Enc' ffbuild/config.log | tail -n 40; exit 1; }}; make -j{j} && make install",
                    j = ctx.jobs
                ),
                &pkg_env,
            )
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
                "-DBUILD_TESTING=OFF",
            ],
        ),
        "jpegli" => cmake(
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
                "-DBUILD_TESTING=OFF",
            ],
        ),
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

/// x265 multilib (8/10-bit in one static library; 12-bit deliberately not built).
fn x265(ctx: &Ctx, src: &Path) -> Result<()> {
    let win = cfg!(windows);
    let prefix = format!("-DCMAKE_INSTALL_PREFIX={}", ctx.prefix.display());
    let mut common = vec![
        "-G",
        "Ninja",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        "-DENABLE_SHARED=OFF",
        "-DENABLE_CLI=OFF",
    ];
    if win {
        common.extend(["-DCMAKE_C_COMPILER=cl", "-DCMAKE_CXX_COMPILER=cl"]);
    } else {
        // x265 4.2 json11 lacks <cstdint>, which recent GCC no longer pulls in transitively.
        common.extend([
            "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
            "-DCMAKE_CXX_FLAGS=-include cstdint",
        ]);
    }
    // MSVC names the archives `x265-static.lib`; elsewhere `libx265.a`.
    let built = if win { "x265-static.lib" } else { "libx265.a" };
    let d10 = src.join("vx10");
    let d8 = src.join("vx8");

    std::fs::create_dir_all(&d10)?;
    let mut args = vec!["../source"];
    args.extend(&common);
    args.extend([
        "-DHIGH_BIT_DEPTH=ON",
        "-DEXPORT_C_API=OFF",
        "-DENABLE_HDR10_PLUS=ON",
        "-DMAIN10=ON",
    ]);
    run(&d10, "cmake", &args, &[])?;
    run(&d10, "ninja", &["-j", &ctx.jobs], &[])?;

    std::fs::create_dir_all(&d8)?;
    let main10 = if win {
        "x265_main10.lib"
    } else {
        "libx265_main10.a"
    };
    std::fs::copy(d10.join(built), d8.join(main10))?;
    let extra_lib = format!("-DEXTRA_LIB={main10}");
    let mut args = vec!["../source"];
    args.extend(&common);
    args.extend([
        &extra_lib,
        "-DEXTRA_LINK_FLAGS=-L.",
        "-DLINKED_10BIT=ON",
        "-DENABLE_HDR10_PLUS=ON",
        &prefix,
    ]);
    run(&d8, "cmake", &args, &[])?;
    run(&d8, "ninja", &["-j", &ctx.jobs], &[])?;
    run(&d8, "ninja", &["install"], &[])?;

    // Merge the 8-bit and 10-bit archives into the installed x265 library.
    let libdir = ctx.prefix.join("lib");
    if win {
        run(
            &d8,
            "lib.exe",
            &["/NOLOGO", "/OUT:x265.lib", built, main10],
            &[],
        )?;
        std::fs::copy(d8.join("x265.lib"), libdir.join("x265.lib"))?;
    } else {
        let script =
            "create libx265.a\naddlib libx265_main.a\naddlib libx265_main10.a\nsave\nend\n";
        std::fs::rename(d8.join("libx265.a"), d8.join("libx265_main.a"))?;
        std::fs::write(d8.join("merge.mri"), script)?;
        run(&d8, "sh", &["-c", "ar -M < merge.mri"], &[])?;
        std::fs::copy(d8.join("libx265.a"), libdir.join("libx265.a"))?;
    }

    // x265 only generates its .pc file for shared builds; FFmpeg's configure needs one.
    let p = unix(&ctx.prefix);
    let private = if win {
        ""
    } else {
        " -lstdc++ -lm -ldl -lpthread"
    };
    let pc = format!(
        "prefix={p}\nlibdir={p}/lib\nincludedir={p}/include\n\nName: x265\n\
         Description: H.265/HEVC video encoder (8/10-bit)\nVersion: 4.2\n\
         Libs: -L${{libdir}} -lx265\nLibs.private: -lhdr10plus{private}\n\
         Cflags: -I${{includedir}}\n"
    );
    std::fs::create_dir_all(libdir.join("pkgconfig"))?;
    std::fs::write(libdir.join("pkgconfig/x265.pc"), pc)?;
    Ok(())
}
