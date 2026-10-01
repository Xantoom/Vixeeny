// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-stitch — see README.md.

/// Crate name, used by the M0 smoke test.
pub const CRATE_NAME: &str = "vixeeny-stitch";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name() {
        assert_eq!(super::CRATE_NAME, "vixeeny-stitch");
    }
}
