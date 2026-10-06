// SPDX-License-Identifier: GPL-3.0-or-later
//! Turns the icon paths of `vixeeny-ui/ui/icons.slint` (Fluent UI System Icons, 20×20 outlines)
//! into Rust constants, so both front-ends draw the very same icons.

use std::fmt::Write as _;

fn main() {
    let source = "../vixeeny-ui/ui/icons.slint";
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read_to_string(source).unwrap_or_else(|e| panic!("{source}: {e}"));
    let mut out = String::new();
    let mut names = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("out property <string> ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        let path = value.trim().trim_end_matches(';').trim_matches('"');
        let name = name.trim().replace('-', "_").to_uppercase();
        writeln!(out, "pub const {name}: &str = {path:?};").unwrap_or_default();
        names.push(name);
    }
    let all: Vec<String> = names.iter().map(|n| format!("({n:?}, {n})")).collect();
    writeln!(
        out,
        "pub const ALL: [(&str, &str); {}] = [{}];",
        names.len(),
        all.join(", ")
    )
    .unwrap_or_default();
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap_or_default()).join("icons.rs");
    std::fs::write(&dest, out).unwrap_or_else(|e| panic!("{}: {e}", dest.display()));
}
