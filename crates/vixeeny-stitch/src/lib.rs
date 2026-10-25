// SPDX-License-Identifier: GPL-3.0-or-later
//! Assembles a long image from frames captured while the user scrolls. Pure code, no OS: tested on synthetic sequences.
//!
//! Method: every row gets a signature (the mean colour of 32 bands of columns). A new frame is placed where its rows'
//! signatures agree with those of the image already assembled, looking first where the scrolling speed says it should
//! be. What stays put on the screen while the content moves (a sticky header, a floating button, blank space) is left
//! out of the comparison. Each row of the image keeps its view from nearest the middle of the screen, so that what
//! floats over the content at the edges is not repeated down the image.

mod frame;
mod stitcher;

pub use frame::Frame;
pub use stitcher::{Config, Direction, Push, StitchError, Stitched, Stitcher};

#[cfg(test)]
mod tests;
