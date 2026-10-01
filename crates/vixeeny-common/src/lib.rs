// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-common — see README.md.

pub mod config;
pub mod hotkey;
pub mod i18n;
pub mod ipc;
pub mod logging;
pub mod naming;
pub mod paths;

/// Crate name, used by the M0 smoke test.
pub const CRATE_NAME: &str = "vixeeny-common";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name() {
        assert_eq!(super::CRATE_NAME, "vixeeny-common");
    }
}
