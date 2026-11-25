// SPDX-License-Identifier: GPL-3.0-or-later
//! Start with the Windows session, kept in line with the `autostart` setting (see `vixeeny_platform::autostart`).

/// Set to anything to leave the OS autostart entry untouched (benchmarks, tests, CI).
pub const DISABLE_ENV: &str = "VIXEENY_NO_AUTOSTART";

/// Whether Vixeeny starts with the session (the installer's choice, before any setting).
pub fn registered() -> bool {
    vixeeny_platform::autostart::registered()
}

/// Makes the OS autostart entry match `enabled`, pointing at the running daemon executable.
pub fn apply(enabled: bool) -> anyhow::Result<()> {
    // The tests run the daemon's logic: they must never point the user's entry at a test program.
    if cfg!(test) || std::env::var_os(DISABLE_ENV).is_some() {
        tracing::debug!("autostart left untouched ({DISABLE_ENV} is set)");
        return Ok(());
    }
    if vixeeny_platform::autostart::apply(&std::env::current_exe()?, enabled)? {
        tracing::info!("autostart {}", if enabled { "enabled" } else { "disabled" });
    }
    Ok(())
}
