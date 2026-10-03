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
    #[cfg(target_os = "linux")]
    if ashpd::is_sandboxed() {
        return portal(enabled);
    }
    let exe = std::env::current_exe()?;
    let mut builder = AutoLaunchBuilder::new();
    builder
        .set_app_name("Vixeeny")
        .set_app_path(&exe.to_string_lossy());
    // A plist in ~/Library/LaunchAgents (`com.vixeeny.Vixeeny`): no login-item permission prompt.
    #[cfg(target_os = "macos")]
    builder.set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent);
    let launcher = builder.build()?;
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

/// Inside Flatpak the autostart file cannot be written by hand: the Background portal makes it
/// (and may ask the user once).
#[cfg(target_os = "linux")]
fn portal(enabled: bool) -> anyhow::Result<()> {
    use ashpd::desktop::background::Background;

    async_io::block_on(
        Background::request()
            .auto_start(enabled)
            .command(["vixeeny-daemon"])
            .reason("Capture shortcuts and the tray icon")
            .send(),
    )?;
    tracing::info!(
        "autostart {} (portal)",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}
