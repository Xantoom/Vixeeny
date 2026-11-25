// SPDX-License-Identifier: GPL-3.0-or-later
//! Values read from the registry, and the few Vixeeny writes.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    RegSetKeyValueW,
};
use windows::core::HSTRING;

/// A string value, `None` when it is missing or blank.
pub(crate) fn string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let mut bytes = 0u32;
    // SAFETY: a size query: no buffer, `bytes` receives the size.
    let status = unsafe { RegGetValueW(root, &key, &value, RRF_RT_REG_SZ, None, None, Some(&raw mut bytes)) };
    if status != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buffer` holds `bytes` bytes; RRF_RT_REG_SZ checks the type.
    let status = unsafe {
        RegGetValueW(root, &key, &value, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&raw mut bytes))
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..len]);
    (!text.trim().is_empty()).then_some(text)
}

/// A `REG_DWORD` value, `None` when it is missing.
pub(crate) fn dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `data` holds `size` bytes; RRF_RT_REG_DWORD checks the type.
    let status = unsafe {
        RegGetValueW(root, &key, &value, RRF_RT_REG_DWORD, None, Some((&raw mut data).cast()), Some(&raw mut size))
    };
    (status == ERROR_SUCCESS).then_some(data)
}

/// Writes a `REG_DWORD` value, making its key when needed.
pub(crate) fn set_dword(root: HKEY, key: &str, value: &str, data: u32) -> Result<(), String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    // SAFETY: `data` holds the 4 bytes given.
    let status = unsafe {
        RegSetKeyValueW(root, &key, &value, REG_DWORD.0, Some((&raw const data).cast()), size_of::<u32>() as u32)
    };
    if status == ERROR_SUCCESS { Ok(()) } else { Err(format!("cannot write {}\\{}: {status:?}", key, value)) }
}

/// Writes a `REG_SZ` value, making its key when needed.
fn set_string(root: HKEY, key: &str, value: &str, data: &str) -> Result<(), String> {
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    let wide: Vec<u16> = data.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = u32::try_from(wide.len() * 2).map_err(|_| "the value is too long".to_owned())?;
    // SAFETY: `wide` holds `bytes` bytes, its terminating zero included.
    let status = unsafe { RegSetKeyValueW(root, &key, &value, REG_SZ.0, Some(wide.as_ptr().cast()), bytes) };
    if status == ERROR_SUCCESS { Ok(()) } else { Err(format!("cannot write {}\\{}: {status:?}", key, value)) }
}

/// The entry of Vixeeny in Windows' installed apps, written by its installer (Inno Setup: the AppId, then `_is1`).
const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{C3B9682C-A50C-4D2D-A615-DB8FE7EC4EFD}_is1";

/// Whether two folders are the same one, written as Windows allows (any case, a trailing separator or not).
fn same_folder(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    !a.trim().is_empty() && norm(a) == norm(b)
}

/// Puts `version` in the version field of the installed apps' entry of the install in `dir`, after an update that
/// replaced the files without the installer. Nothing when no entry is of this folder (a portable copy). `Ok(true)` when
/// the field changed. An entry for all users needs the administrator's rights, which an update in Program Files has.
pub fn sync_uninstall_version(dir: &std::path::Path, version: &str) -> Result<bool, String> {
    let dir = dir.to_string_lossy();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let Some(path) = string(root, UNINSTALL_KEY, "Inno Setup: App Path") else {
            continue;
        };
        if !same_folder(&path, &dir) {
            continue;
        }
        if string(root, UNINSTALL_KEY, "DisplayVersion").as_deref() == Some(version) {
            return Ok(false);
        }
        set_string(root, UNINSTALL_KEY, "DisplayVersion", version)?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn folders_compare_as_windows_does() {
        assert!(super::same_folder(r"C:\Program Files\Vixeeny", r"c:\program files\vixeeny\"));
        assert!(super::same_folder("C:/Users/a/Vixeeny", r"C:\Users\a\Vixeeny"));
        assert!(!super::same_folder(r"C:\Program Files\Vixeeny", r"C:\Program Files\Vixeeny2"));
        assert!(!super::same_folder("", ""));
    }

    #[test]
    fn a_string_written_is_read_back() {
        use windows::Win32::System::Registry::RegDeleteTreeW;
        use windows::core::HSTRING;
        let key = format!(r"Software\Vixeeny-test-string-{}", std::process::id());
        let written = super::set_string(super::HKEY_CURRENT_USER, &key, "Value", "1.0.0-RC.3");
        let read = super::string(super::HKEY_CURRENT_USER, &key, "Value");
        // SAFETY: a key this test made.
        let _ = unsafe { RegDeleteTreeW(super::HKEY_CURRENT_USER, &HSTRING::from(key.as_str())) };
        assert_eq!(written, Ok(()));
        assert_eq!(read.as_deref(), Some("1.0.0-RC.3"));
    }

    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
    use windows::core::HSTRING;

    /// Written, read back, then the key goes: nothing of the test stays in the user's registry.
    #[test]
    fn a_dword_written_is_read_back() {
        let key = format!(r"Software\Vixeeny-test-{}", std::process::id());
        let written = super::set_dword(HKEY_CURRENT_USER, &key, "Value", 7);
        let read = super::dword(HKEY_CURRENT_USER, &key, "Value");
        // SAFETY: a key this test made.
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str())) };
        assert_eq!(written, Ok(()));
        assert_eq!(read, Some(7));
    }
}
