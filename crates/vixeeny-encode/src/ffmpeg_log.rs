// SPDX-License-Identifier: GPL-3.0-or-later
//! FFmpeg's error messages go to Vixeeny's log (its own output, stderr, has no console to show
//! it): an encoder or driver that fails on a user's machine says why in the bug report.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Once;

use ffmpeg_next::ffi;

/// Sends FFmpeg's errors to the log from now on (once per process; later calls do nothing).
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: plain setters of FFmpeg's global logging state; `callback` is thread safe.
        unsafe {
            ffi::av_log_set_level(ffi::AV_LOG_ERROR);
            ffi::av_log_set_callback(Some(callback));
        }
    });
}

unsafe extern "C" fn callback(
    avcl: *mut c_void,
    level: c_int,
    fmt: *const c_char,
    args: ffi::va_list,
) {
    if level > ffi::AV_LOG_ERROR {
        return;
    }
    let mut line = [0 as c_char; 1024];
    let mut prefix = 1;
    // SAFETY: the arguments come from FFmpeg as they are; `line` holds `line.len()` chars and
    // is NUL-terminated by the call (which truncates).
    let text = unsafe {
        ffi::av_log_format_line2(
            avcl,
            level,
            fmt,
            args,
            line.as_mut_ptr(),
            line.len() as c_int,
            &raw mut prefix,
        );
        CStr::from_ptr(line.as_ptr())
    };
    let text = text.to_string_lossy();
    let text = text.trim_end();
    if !text.is_empty() {
        tracing::error!(target: "ffmpeg", "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_messages_with_arguments_are_formatted() {
        install();
        install();
        // SAFETY: a format string and the argument it names.
        unsafe {
            ffi::av_log(
                std::ptr::null_mut(),
                ffi::AV_LOG_ERROR,
                c"test error %d: %s\n".as_ptr(),
                42 as c_int,
                c"formatted".as_ptr(),
            );
            // Below the level: dropped before formatting.
            ffi::av_log(std::ptr::null_mut(), ffi::AV_LOG_INFO, c"info\n".as_ptr());
        }
    }
}
