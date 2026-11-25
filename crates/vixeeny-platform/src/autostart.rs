// SPDX-License-Identifier: GPL-3.0-or-later
//! Start with the Windows session: the `Vixeeny` value of `HKCU\…\Run`. Only this user's entry, and only the entry: the
//! choice made in Task Manager (its "Startup apps") stays Windows' own. The daemon keeps it in line with its setting
//! (and the program's path); the installer has the app write it, as the user who installs.

use windows::Win32::System::Registry::HKEY_CURRENT_USER;

use crate::registry;
use crate::{PlatformError, Result};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "Vixeeny";

/// What the entry starts now (`None`: there is none).
pub fn entry() -> Option<String> {
    registry::string(HKEY_CURRENT_USER, RUN, NAME)
}

/// Whether Vixeeny starts with the session (the installer's choice, before any setting).
pub fn registered() -> bool {
    entry().is_some()
}

/// The entry for `exe`: quoted, as a path with spaces (a user name) needs.
fn command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Makes the entry match `enabled`, starting `exe` (the daemon). `Ok(true)` when it changed.
pub fn apply(exe: &std::path::Path, enabled: bool) -> Result<bool> {
    let current = entry();
    let wanted = enabled.then(|| command(exe));
    if current == wanted {
        return Ok(false);
    }
    let written = match &wanted {
        // The entry follows the program when its path or name changes.
        Some(wanted) => registry::set_string(HKEY_CURRENT_USER, RUN, NAME, wanted),
        None => registry::delete_value(HKEY_CURRENT_USER, RUN, NAME),
    };
    written.map_err(PlatformError::Os)?;
    // Read back: a write that did not land (a policy, a security program) is said rather than taken for done.
    if entry() != wanted {
        return Err(PlatformError::Os("the autostart entry did not change".into()));
    }
    Ok(true)
}

/// Points an existing entry at `exe` (an update or a move changed the program's path); no entry stays none.
pub fn refresh(exe: &std::path::Path) -> Result<bool> {
    if registered() { apply(exe, true) } else { Ok(false) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_entry_is_quoted_for_paths_with_spaces() {
        let exe = std::path::Path::new(r"C:\Users\Jean Dupont\AppData\Local\Programs\Vixeeny\Vixeeny.exe");
        assert_eq!(super::command(exe), r#""C:\Users\Jean Dupont\AppData\Local\Programs\Vixeeny\Vixeeny.exe""#);
    }
}
