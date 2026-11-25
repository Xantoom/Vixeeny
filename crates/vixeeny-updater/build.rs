// SPDX-License-Identifier: GPL-3.0-or-later
//! The notes of this version (`changelog/<version>/en.md` and `fr.md`), built into the program: what changed shows at
//! once, without the network, in the user's language. A version without notes (a development build) has none.

use std::path::PathBuf;

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let dir = manifest.join("../../changelog").join(&version);
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut out = String::new();
    for (lang, constant) in [("en", "EN"), ("fr", "FR")] {
        let file = dir.join(format!("{lang}.md"));
        println!("cargo:rerun-if-changed={}", file.display());
        let value = match file.canonicalize() {
            Ok(path) => format!("Some(include_str!({:?}))", path.display().to_string()),
            Err(_) => "None".to_owned(),
        };
        out.push_str(&format!("pub const {constant}: Option<&str> = {value};\n"));
    }
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("changelog.rs");
    if let Err(e) = std::fs::write(&dest, out) {
        println!("cargo:warning=cannot write {}: {e}", dest.display());
    }
}
