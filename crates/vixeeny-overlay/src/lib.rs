// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-overlay — the zone editor of Print Screen, drawn natively (see the README).

pub mod gfx;
pub mod icons {
    //! The icons of `vixeeny-ui/ui/icons.slint`, as SVG path data (20×20).
    include!(concat!(env!("OUT_DIR"), "/icons.rs"));
}
pub mod layout;
mod overlay;
pub mod paint;
mod scene;
pub mod svg;
pub mod theme;
mod window;

pub use overlay::{Overlay, OverlayError, Screen, post};
pub use theme::Look;

#[cfg(test)]
mod tests;
