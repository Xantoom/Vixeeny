// SPDX-License-Identifier: GPL-3.0-or-later
//! Placeholder for the platforms that arrive after Windows (M21/M22).

use super::Startup;

pub fn run(_startup: Startup) -> anyhow::Result<()> {
    anyhow::bail!("the Vixeeny daemon is not supported on this platform yet")
}
