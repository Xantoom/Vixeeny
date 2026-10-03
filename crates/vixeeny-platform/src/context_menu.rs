// SPDX-License-Identifier: GPL-3.0-or-later
//! The "Convert with Vixeeny" entry of the file manager's context menu (plan 5.5). On Windows it
//! is a per-user registry verb on every supported image extension (no administrator rights).

use std::path::Path;

/// Extensions that get the entry (the readable formats of `vixeeny-image`).
pub const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "webp", "avif", "jxl", "bmp", "tif", "tiff", "gif",
];

/// Name of the verb key.
pub const VERB: &str = "VixeenyConvert";

/// One registry key under `HKEY_CURRENT_USER` and its string values (`""` is the default value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub values: Vec<(&'static str, String)>,
}

/// Every key to create. `label` is the menu text; `exe` the `vixeeny-app` executable.
pub fn entries(exe: &Path, label: &str) -> Vec<Entry> {
    let exe = exe.display();
    let mut out = Vec::new();
    for ext in EXTENSIONS {
        let verb = format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\{VERB}");
        out.push(Entry {
            key: verb.clone(),
            values: vec![
                ("", label.to_owned()),
                ("Icon", format!("\"{exe}\",0")),
                // All selected files in one command line.
                ("MultiSelectModel", "Player".to_owned()),
            ],
        });
        out.push(Entry {
            key: format!("{verb}\\command"),
            values: vec![("", format!("\"{exe}\" --convert \"%1\""))],
        });
    }
    out
}

/// Keys to delete (their sub-keys go with them).
pub fn verb_keys() -> Vec<String> {
    EXTENSIONS
        .iter()
        .map(|ext| format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\{VERB}"))
        .collect()
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows::core::PCWSTR;

    use super::{Entry, verb_keys};
    use crate::PlatformError;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn os(what: &str, code: impl std::fmt::Display) -> PlatformError {
        PlatformError::Os(format!("registry {what}: {code}"))
    }

    pub fn write(entries: &[Entry]) -> crate::Result<()> {
        for entry in entries {
            let key = wide(&entry.key);
            let mut handle = HKEY::default();
            // SAFETY: `key` is NUL-terminated and outlives the call; `handle` receives the key.
            let status = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    None,
                    &mut handle,
                    None,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(os("create", status.0));
            }
            let mut result = Ok(());
            for (name, value) in &entry.values {
                let name_w = wide(name);
                let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
                let name_ptr = if name.is_empty() {
                    PCWSTR::null()
                } else {
                    PCWSTR(name_w.as_ptr())
                };
                // SAFETY: the key is open for writing; `data` is a NUL-terminated UTF-16 string.
                let status = unsafe { RegSetValueExW(handle, name_ptr, None, REG_SZ, Some(&data)) };
                if status != ERROR_SUCCESS {
                    result = Err(os("set", status.0));
                    break;
                }
            }
            // SAFETY: the key was opened above and is closed once.
            let _ = unsafe { RegCloseKey(handle) };
            result?;
        }
        Ok(())
    }

    pub fn remove() -> crate::Result<()> {
        for key in verb_keys() {
            let key = wide(&key);
            // SAFETY: `key` is NUL-terminated and outlives the call.
            let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                return Err(os("delete", status.0));
            }
        }
        Ok(())
    }

    pub fn installed() -> bool {
        let Some(first) = verb_keys().into_iter().next() else {
            return false;
        };
        let key = wide(&first);
        let mut handle = HKEY::default();
        // SAFETY: `key` is NUL-terminated and outlives the call; `handle` receives the key.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                None,
                KEY_READ,
                &mut handle,
            )
        };
        if status == ERROR_SUCCESS {
            // SAFETY: the key was opened above and is closed once.
            let _ = unsafe { RegCloseKey(handle) };
            true
        } else {
            false
        }
    }
}

/// Adds the entry for the current user.
#[cfg(windows)]
pub fn install(exe: &Path, label: &str) -> crate::Result<()> {
    imp::write(&entries(exe, label))
}

/// Writes arbitrary per-user registry entries (used by the native notifications).
#[cfg(windows)]
pub(crate) fn write_entries(entries: &[Entry]) -> crate::Result<()> {
    imp::write(entries)
}

/// Removes the entry (a no-op when it is absent).
#[cfg(windows)]
pub fn uninstall() -> crate::Result<()> {
    imp::remove()
}

#[cfg(windows)]
pub fn is_installed() -> bool {
    imp::installed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_extension_gets_a_verb_and_a_quoted_command() {
        let e = entries(
            Path::new("C:\\Program Files\\Vixeeny\\vixeeny-app.exe"),
            "Convert",
        );
        assert_eq!(e.len(), EXTENSIONS.len() * 2);
        let command = e
            .iter()
            .find(|e| e.key.ends_with(".png\\shell\\VixeenyConvert\\command"))
            .unwrap();
        assert_eq!(
            command.values[0].1,
            "\"C:\\Program Files\\Vixeeny\\vixeeny-app.exe\" --convert \"%1\""
        );
        assert!(e.iter().all(|e| e.key.starts_with("Software\\Classes\\")));
        assert_eq!(verb_keys().len(), EXTENSIONS.len());
    }
}
