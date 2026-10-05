// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-ui — the Slint user interface (plan 4.1): `.slint` files and their Rust binding.

// Code generated from the `.slint` files: not ours to lint.
#[allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::undocumented_unsafe_blocks,
    unsafe_op_in_unsafe_fn
)]
mod generated {
    slint::include_modules!();
}
pub use generated::*;

pub mod ocr_panel;
pub mod overlay;
pub mod scroll_panel;
pub mod settings_panel;
pub mod side_panel;
pub mod theme;
pub mod toast_panel;
pub mod widget_panel;
pub mod wizard_panel;
pub use overlay::{Overlay, Screen};
pub use slint;
pub use slint::ComponentHandle;
pub use vixeeny_ocr::OcrPanel;

#[cfg(test)]
mod tests;
