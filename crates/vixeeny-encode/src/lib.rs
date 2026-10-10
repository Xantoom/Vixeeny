// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-encode — the codec registry (plan 6), profile validation, hardware probing and, in
//! later milestones, the video pipeline. See README.md.

pub mod clock;
mod convert;
#[cfg(windows)]
pub mod d3d_convert;
#[cfg(windows)]
pub mod driver_caps;
pub mod hdr;
pub mod probe;
pub mod registry;
pub mod validate;

pub mod audio;
pub mod denoise;
pub mod ffmpeg_log;
pub mod ffmpeg_probe;
pub mod gpu;
pub mod recorder;
pub mod replay;
pub mod thumbnail;

/// FFmpeg bindings, built against the pinned prebuilt FFmpeg of `native/versions.toml`.
pub use ffmpeg_next as ffmpeg;

/// Crate name, used by the M0 smoke test.
pub const CRATE_NAME: &str = "vixeeny-encode";

#[cfg(test)]
mod tests;
