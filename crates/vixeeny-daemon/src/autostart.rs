// SPDX-License-Identifier: GPL-3.0-or-later
//! Start with the OS session (plan 5.15): `HKCU\…\Run` on Windows, LaunchAgent on macOS,
//! autostart `.desktop` file on Linux — all through the `auto-launch` crate.

use auto_launch::AutoLaunchBuilder;

/// Set to anything to leave the OS autostart entry untouched (benchmarks, tests, CI).
pub const DISABLE_ENV: &str = "VIXEENY_NO_AUTOSTART";

/// Makes the OS autostart entry match `enabled`, pointing at the running daemon executable.
pub fn apply(enabled: bool) -> anyhow::Result<()> {
    if std::env::var_os(DISABLE_ENV).is_some() {
        tracing::debug!("autostart left untouched ({DISABLE_ENV} is set)");
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let launcher = AutoLaunchBuilder::new()
        .set_app_name("Vixeeny")
        .set_app_path(&exe.to_string_lossy())
        .build()?;
    let current = launcher.is_enabled()?;
    if enabled && !current {
        launcher.enable()?;
        tracing::info!("autostart enabled");
    } else if !enabled && current {
        launcher.disable()?;
        tracing::info!("autostart disabled");
    }
    Ok(())
}
