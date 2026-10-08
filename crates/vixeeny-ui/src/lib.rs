// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-ui — the Slint user interface (plan 4.1): `.slint` files and their Rust binding. The
//! windows shown often (side strip, notifications, recording widget) are native: see
//! `vixeeny-overlay`.

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

pub mod scroll_panel;
pub mod settings_panel;
pub mod theme;
pub mod wizard_panel;
pub use slint;
pub use slint::ComponentHandle;

#[cfg(test)]
mod tests;
