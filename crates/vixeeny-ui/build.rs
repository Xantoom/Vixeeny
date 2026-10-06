// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(clippy::expect_used)]

fn main() {
    slint_build::compile("ui/app.slint").expect("slint compilation");
}
