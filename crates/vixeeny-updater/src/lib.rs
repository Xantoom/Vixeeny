// SPDX-License-Identifier: GPL-3.0-or-later
//! Update check, download, verification and replacement (plan 10.2). The pure parts (versions,
//! release metadata, signatures, file swap with rollback) are tested on their own; `client`
//! wires them to the network and to the running programs.

#[cfg(feature = "net")]
pub mod client;
pub mod release;
pub mod state;
pub mod swap;
pub mod verify;

pub use release::{Release, check_url, is_newer};
