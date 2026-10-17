// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-encode — the codec registry, profile validation, hardware probing and, in
//! later milestones, the video pipeline. See README.md.

#[cfg(windows)]
mod card_scale;
pub mod clock;
mod convert;
pub mod cut;
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

#[cfg(test)]
mod tests;
