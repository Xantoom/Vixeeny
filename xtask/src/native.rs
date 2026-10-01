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
    if cfg!(windows) {
        bail!("build-native on Windows runs inside MSYS2; see native/build/README.md");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()?;
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
        if std::fs::read_to_string(&stamp).is_ok_and(|s| s == lib.commit) {
            println!("== {name}: up to date");
            continue;
        }
        println!("== {name}: fetching {}", lib.commit);
        let src = fetch(&ctx, name, lib)?;
        println!("== {name}: building");
        recipe(&ctx, name, &src)?;
        std::fs::write(&stamp, &lib.commit)?;
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
    args.extend_from_slice(extra);
    run(src, "cmake", &args, &[])?;
    let build = build.to_string_lossy().into_owned();
    run(src, "cmake", &["--build", &build, "-j", &ctx.jobs], &[])?;
    run(src, "cmake", &["--install", &build], &[])
}

fn recipe(ctx: &Ctx, name: &str, src: &Path) -> Result<()> {
    let prefix = ctx.prefix.display().to_string();
    let pc = ctx.prefix.join("lib/pkgconfig").display().to_string();
    let pc2 = ctx.prefix.join("lib64/pkgconfig").display().to_string();
    let pkg_env = [("PKG_CONFIG_PATH", format!("{pc}:{pc2}"))];
    let make = |dir: &Path| -> Result<()> {
        run(dir, "make", &["-j", &ctx.jobs], &[])?;
        run(dir, "make", &["install"], &[])
    };
    match name {
        "x264" => {
            run(
                src,
                "./configure",
                &[
                    &format!("--prefix={prefix}"),
                    "--enable-static",
                    "--enable-pic",
                    "--disable-cli",
                    "--disable-opencl",
                ],
                &[],
            )?;
            make(src)
        }
        "x265" => x265(ctx, src),
        "libvpx" => {
            run(
                src,
                "./configure",
                &[
                    &format!("--prefix={prefix}"),
                    "--enable-static",
                    "--disable-shared",
                    "--enable-pic",
                    "--enable-vp9-highbitdepth",
                    "--disable-examples",
                    "--disable-tools",
                    "--disable-docs",
                    "--disable-unit-tests",
                ],
                &[],
            )?;
            make(src)
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
            run(
                src,
                "./configure",
                &[
                    &format!("--prefix={prefix}"),
                    "--enable-gpl",
                    "--enable-version3",
                    "--enable-static",
                    "--disable-shared",
                    "--enable-pic",
                    "--disable-programs",
                    "--disable-doc",
                    "--disable-debug",
                    "--pkg-config-flags=--static",
                    "--enable-libx264",
                    "--enable-libx265",
                    "--enable-libvpx",
                    "--enable-libsvtav1",
                    "--enable-libdav1d",
                    "--enable-libopus",
                ],
                &pkg_env,
            )?;
            make(src)
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
    let (dir, bits) = ("vx10", "MAIN10");
    std::fs::create_dir_all(src.join(dir))?;
    let main = format!("-D{bits}=ON");
    let mut args = vec!["../source"];
    args.extend(common);
    args.extend([
        "-DHIGH_BIT_DEPTH=ON",
        "-DEXPORT_C_API=OFF",
        "-DENABLE_HDR10_PLUS=ON",
        &main,
    ]);
    run(&src.join(dir), "cmake", &args, &[])?;
    run(&src.join(dir), "ninja", &["-j", &ctx.jobs], &[])?;
    std::fs::create_dir_all(src.join("vx8"))?;
    let d8 = src.join("vx8");
    run(
        &d8,
        "ln",
        &["-sf", "../vx10/libx265.a", "libx265_main10.a"],
        &[],
    )?;
    let mut args = vec!["../source"];
    args.extend(common);
    args.extend([
        "-DEXTRA_LIB=x265_main10.a",
        "-DEXTRA_LINK_FLAGS=-L.",
        "-DLINKED_10BIT=ON",
        "-DENABLE_HDR10_PLUS=ON",
        &prefix,
    ]);
    run(&d8, "cmake", &args, &[])?;
    run(&d8, "ninja", &["-j", &ctx.jobs], &[])?;
    // Merge the three archives into the installed libx265.a.
    let script = "create libx265.a\naddlib libx265_main.a\naddlib libx265_main10.a\nsave\nend\n";
    run(&d8, "mv", &["libx265.a", "libx265_main.a"], &[])?;
    std::fs::write(d8.join("merge.mri"), script)?;
    run(&d8, "sh", &["-c", "ar -M < merge.mri"], &[])?;
    run(&d8, "ninja", &["install"], &[])?;
    // x265 only generates its .pc file for shared builds; FFmpeg's configure needs one.
    let libdir = ctx.prefix.join("lib");
    let pc = format!(
        "prefix={p}\nlibdir={p}/lib\nincludedir={p}/include\n\nName: x265\n\
         Description: H.265/HEVC video encoder (8/10-bit)\nVersion: 4.2\n\
         Libs: -L${{libdir}} -lx265\nLibs.private: -lhdr10plus -lstdc++ -lm -ldl -lpthread\n\
         Cflags: -I${{includedir}}\n",
        p = ctx.prefix.display()
    );
    std::fs::write(libdir.join("pkgconfig/x265.pc"), pc)?;
    Ok(())
}
