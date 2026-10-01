// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-image — image encoding (plan 5.4). M4 ships the fast PNG path used by direct
//! captures; JPEG, WebP, AVIF, JPEG XL, ICC and HDR arrive with M5.

pub mod png_fast;

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("invalid pixel buffer: {0}")]
    BadBuffer(&'static str),
    #[error("png: {0}")]
    Png(#[from] png::EncodingError),
}
