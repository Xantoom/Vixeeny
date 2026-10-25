// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-common — see README.md.

pub mod buffers;
pub mod colour;
pub mod config;
pub mod half;
pub mod hotkey;
pub mod i18n;
pub mod ipc;
pub mod logging;
pub mod look;
pub mod machine;
pub mod naming;
pub mod paths;
pub mod time;

/// A test that needs hardware this machine may lack (a monitor, a graphics card, an encoder)
/// gives up through here: the reason is printed, and with `VIXEENY_REAL_HARDWARE` set (the
/// developer's machine, which has it all) giving up is a failure instead.
#[doc(hidden)]
pub fn skip_test(reason: impl std::fmt::Display) {
    assert!(
        std::env::var_os("VIXEENY_REAL_HARDWARE").is_none(),
        "this machine should not skip: {reason}"
    );
    eprintln!("skipped: {reason}");
}
