// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used)]
fn main() {
    slint_build::compile("ui/editor.slint").unwrap_or_else(|e| panic!("slint: {e}"));
}
