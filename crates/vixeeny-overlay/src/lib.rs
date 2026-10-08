// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-overlay — what Vixeeny draws natively (see the README): the zone editor of Print
//! Screen, the side strip, the notifications and the recording widget.

pub mod gfx;
pub mod icons {
    //! The icons of `vixeeny-ui/ui/icons.slint`, as SVG path data (20×20).
    include!(concat!(env!("OUT_DIR"), "/icons.rs"));
}
pub mod layout;
mod overlay;
pub mod paint;
pub mod popup;
mod scene;
pub mod side;
pub mod svg;
pub mod theme;
pub mod toast;
pub mod widget;
mod window;

pub use overlay::{Overlay, OverlayError, Screen, post};
pub use theme::{Look, dark_theme};

#[cfg(test)]
mod tests;
