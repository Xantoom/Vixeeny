// SPDX-License-Identifier: GPL-3.0-or-later
//! Values read from the registry.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
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
