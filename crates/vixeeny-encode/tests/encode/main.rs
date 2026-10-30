// SPDX-License-Identifier: GPL-3.0-or-later
//! The tests against the real FFmpeg, in one program: linked once instead of once per file.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test helpers fail the test")]

mod common;
mod cut;
mod gpu_path;
mod recorder;
mod software_encode;
mod verify_registry;
