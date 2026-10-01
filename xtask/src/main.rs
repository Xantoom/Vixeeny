// SPDX-License-Identifier: GPL-3.0-or-later
//! Build and maintenance tasks: `cargo xtask <task>`.

use anyhow::{Result, bail};

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some(
            "build-native" | "dist" | "verify-registry" | "bench-idle" | "bench-latency"
            | "test-hw",
        ) => {
            bail!("task not implemented yet (see VIXEENY_PLAN.md section 11)")
        }
        _ => bail!(
            "usage: cargo xtask <build-native|dist|verify-registry|bench-idle|bench-latency|test-hw>"
        ),
    }
}
