// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::OnceLock;

use moxcms::ColorProfile;

use crate::ImageError;

/// The standard sRGB profile, built once.
pub(crate) fn srgb() -> Result<&'static [u8], ImageError> {
    static PROFILE: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    PROFILE
        .get_or_init(|| ColorProfile::new_srgb().encode().map_err(|e| e.to_string()))
        .as_deref()
        .map_err(|e| ImageError::Icc(e.clone()))
}
