// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-encode — the codec registry (plan 6), profile validation, hardware probing and, in
//! later milestones, the video pipeline. See README.md.

pub mod clock;
#[cfg(windows)]
pub mod d3d_convert;
pub mod hdr;
pub mod probe;
pub mod registry;
pub mod validate;

#[cfg(feature = "ffmpeg-next")]
pub mod audio;
#[cfg(feature = "ffmpeg-next")]
pub mod denoise;
#[cfg(feature = "ffmpeg-next")]
pub mod ffmpeg_probe;
#[cfg(feature = "ffmpeg-next")]
pub mod gpu;
#[cfg(feature = "ffmpeg-next")]
#[cfg(feature = "ffmpeg-next")]
pub mod recorder;
#[cfg(feature = "ffmpeg-next")]
pub mod replay;
#[cfg(feature = "ffmpeg-next")]
pub mod thumbnail;

/// FFmpeg bindings, built against the pinned static libraries of `native/versions.toml`.
#[cfg(feature = "ffmpeg-next")]
pub use ffmpeg_next as ffmpeg;

/// Crate name, used by the M0 smoke test.
pub const CRATE_NAME: &str = "vixeeny-encode";

#[cfg(test)]
mod tests;
