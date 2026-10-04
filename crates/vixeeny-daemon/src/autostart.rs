// SPDX-License-Identifier: GPL-3.0-or-later
//! Start with the Windows session (plan 5.15): the `HKCU\…\Run` entry, through `auto-launch`.

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
    let mut builder = AutoLaunchBuilder::new();
    builder
        .set_app_name("Vixeeny")
        .set_app_path(&exe.to_string_lossy());
    let launcher = builder.build()?;
    let current = launcher.is_enabled()?;
    if enabled {
        // Rewritten every time: the entry follows the program when its path or name changes
        // (`vixeeny-daemon.exe` before 1.0).
        launcher.enable()?;
        if !current {
            tracing::info!("autostart enabled");
        }
    } else if current {
        launcher.disable()?;
        tracing::info!("autostart disabled");
    }
    Ok(())
}
