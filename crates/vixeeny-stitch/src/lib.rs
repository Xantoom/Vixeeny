// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-stitch — assembles a long image from frames captured while the user scrolls
//! (plan 5.7). Pure code, no OS: tested on synthetic sequences (CA-SCR-1).
//!
//! Method: each new frame is compared with the previous one. Rows that are identical at the
//! same position in both frames at the top and the bottom are a sticky header/footer; they are
//! excluded from the search and kept once. In the rest, rows are fingerprinted and vote for a
//! vertical shift, which is then verified on the whole overlap (with a small per-pixel
//! tolerance). When no row fingerprint agrees (noise), a coarse luminance comparison is used.

mod frame;
mod shift;
mod stitcher;

pub use frame::Frame;
pub use stitcher::{Config, Direction, Push, StitchError, Stitched, Stitcher};

#[cfg(test)]
mod tests;
