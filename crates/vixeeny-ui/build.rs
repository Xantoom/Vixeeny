// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used, reason = "a build script stops the build on failure")]

fn main() {
    slint_build::compile("ui/app.slint").expect("slint compilation");
}
