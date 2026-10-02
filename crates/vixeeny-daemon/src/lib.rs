// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-daemon — see README.md.
//!
//! The daemon sleeps in the OS message loop. Everything that can wake it (tray clicks, IPC
//! clients, child-process exit) is turned into an [`core::Event`] and handed to the pure state
//! machine in [`core`]; the resulting [`core::Effect`]s are executed by [`runtime::Runtime`].

pub mod autostart;
pub mod core;
pub mod icon;
pub mod platform;
pub mod runtime;
pub mod server;
pub mod supervisor;
pub mod update_check;
