// SPDX-License-Identifier: GPL-3.0-or-later
//! Start with the Windows session: the `Vixeeny` value of `HKCU\…\Run`, the one the installer writes. Only this user's
//! entry, and only the entry: the choice made in Task Manager (its "Startup apps") stays Windows' own.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::w;

/// Set to anything to leave the OS autostart entry untouched (benchmarks, tests, CI).
pub const DISABLE_ENV: &str = "VIXEENY_NO_AUTOSTART";

const RUN: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const NAME: windows::core::PCWSTR = w!("Vixeeny");

fn check(error: WIN32_ERROR, what: &str) -> anyhow::Result<()> {
    anyhow::ensure!(error == ERROR_SUCCESS, "{what}: {}", error.to_hresult().message());
    Ok(())
}

/// What the entry starts now (`None`: there is none).
fn entry() -> anyhow::Result<Option<String>> {
    let mut size = 0_u32;
    // SAFETY: constant names; asks for the size only.
    let error = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, NAME, RRF_RT_REG_SZ, None, None, Some(&raw mut size)) };
    if error == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(error, "read the autostart entry")?;
    let mut text = vec![0_u16; (size as usize).div_ceil(2)];
    // SAFETY: `text` holds `size` bytes.
    let error = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN,
            NAME,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    check(error, "read the autostart entry")?;
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    Ok(Some(String::from_utf16_lossy(&text[..end])))
}

/// Whether Vixeeny starts with the session (the installer's choice, before any setting).
pub fn registered() -> bool {
    entry().is_ok_and(|e| e.is_some())
}

/// The entry for `exe`: quoted, as a path with spaces (a user name) needs.
fn command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Makes the OS autostart entry match `enabled`, pointing at the running daemon executable.
pub fn apply(enabled: bool) -> anyhow::Result<()> {
    // The tests run the daemon's logic: they must never point the user's entry at a test program.
    if cfg!(test) || std::env::var_os(DISABLE_ENV).is_some() {
        tracing::debug!("autostart left untouched ({DISABLE_ENV} is set)");
        return Ok(());
    }
    let current = entry()?;
    if enabled {
        // The entry follows the program when its path or name changes.
        let wanted = command(&std::env::current_exe()?);
        if current.as_deref() == Some(wanted.as_str()) {
            return Ok(());
        }
        let data: Vec<u16> = wanted.encode_utf16().chain([0]).collect();
        // SAFETY: `data` is a terminated wide string of the size given.
        let error = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, RUN, NAME, REG_SZ.0, Some(data.as_ptr().cast()), (data.len() * 2) as u32)
        };
        check(error, "write the autostart entry")?;
        if current.is_none() {
            tracing::info!("autostart enabled");
        }
    } else if current.is_some() {
        // SAFETY: constant names.
        let error = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME) };
        check(error, "remove the autostart entry")?;
        tracing::info!("autostart disabled");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_entry_is_quoted_for_paths_with_spaces() {
        let exe = std::path::Path::new(r"C:\Users\Jean Dupont\AppData\Local\Programs\Vixeeny\Vixeeny.exe");
        assert_eq!(super::command(exe), r#""C:\Users\Jean Dupont\AppData\Local\Programs\Vixeeny\Vixeeny.exe""#);
    }
}
