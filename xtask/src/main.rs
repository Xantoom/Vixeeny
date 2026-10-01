// SPDX-License-Identifier: GPL-3.0-or-later
//! Build and maintenance tasks: `cargo xtask <task>`.

mod bench;
mod native;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("build-native") => native::build(&args.collect::<Vec<_>>()),
        Some("bench-idle") => bench::run(&args.collect::<Vec<_>>()),
        Some("dist" | "verify-registry" | "bench-latency" | "test-hw") => {
            bail!("task not implemented yet (see VIXEENY_PLAN.md section 11)")
        }
        _ => bail!(
            "usage: cargo xtask <build-native [lib…]|dist|verify-registry|bench-idle|bench-latency|test-hw>"
        ),
    }
}
