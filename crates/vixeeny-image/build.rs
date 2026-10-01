// SPDX-License-Identifier: GPL-3.0-or-later
//! With the `native-codecs` feature: find the natively built libavif / libjxl through
//! pkg-config (static) and generate their bindings.
#![allow(clippy::expect_used)]

fn main() {
    #[cfg(feature = "native-codecs")]
    native();
}

#[cfg(feature = "native-codecs")]
fn native() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    let mut includes = Vec::new();
    for lib in ["libavif", "libjxl", "libjxl_threads", "libjpegli"] {
        let found = pkg_config::Config::new()
            .statik(true)
            .probe(lib)
            .unwrap_or_else(|e| {
                panic!("{lib} not found ({e}); run `cargo xtask build-native` and set PKG_CONFIG_PATH to its prefix/lib/pkgconfig")
            });
        includes.extend(found.include_paths);
    }
    let mut builder = bindgen::Builder::default()
        .header_contents(
            "wrapper.h",
            "#include <avif/avif.h>\n#include <jxl/encode.h>\n#include <jxl/decode.h>\n#include <jxl/thread_parallel_runner.h>\n#include <stdio.h>\n#include <jpeglib.h>\n",
        )
        .allowlist_function("avif.*")
        .allowlist_type("avif.*")
        .allowlist_var("AVIF_.*")
        .allowlist_function("Jxl.*")
        .allowlist_type("Jxl.*")
        .allowlist_var("JXL_.*")
        .allowlist_type("j_compress_ptr|jpeg_.*|J[A-Z_]+|JSAMPARRAY|boolean")
        .allowlist_var("JPEG_LIB_VERSION|JCS_.*|TRUE|FALSE")
        .derive_default(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    for dir in includes {
        builder = builder.clang_arg(format!("-I{}", dir.display()));
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    builder
        .generate()
        .expect("bindgen failed on the libavif/libjxl headers")
        .write_to_file(out.join("native.rs"))
        .expect("writing bindings");
}
