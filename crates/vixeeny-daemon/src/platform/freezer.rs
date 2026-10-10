// SPDX-License-Identifier: GPL-3.0-or-later
//! The frozen screens of a zone capture, made by the daemon on its own thread (it has the
//! message loop the windows need) with a Direct3D device kept ready.

use std::time::Instant;

use vixeeny_capture::freeze::{self, Gpu};
use vixeeny_common::ipc::Frozen;

use crate::runtime::Freezer;

pub struct GpuFreezer {
    gpu: Option<Gpu>,
}

impl GpuFreezer {
    /// Creates the device now, so the first press is as fast as the next ones.
    pub fn new() -> Self {
        Self {
            gpu: Self::device(),
        }
    }

    fn device() -> Option<Gpu> {
        Gpu::new()
            .map_err(|e| tracing::warn!("no device for the frozen screens: {e}"))
            .ok()
    }
}

impl Freezer for GpuFreezer {
    fn freeze(&mut self) -> Option<Frozen> {
        if freeze::is_active() {
            return None;
        }
        let started = Instant::now();
        let monitors = vixeeny_platform::monitors()
            .map_err(|e| tracing::warn!("cannot list the monitors: {e}"))
            .ok()?;
        // A second try on a new device: the old one may have been lost (driver update, sleep).
        for _ in 0..2 {
            if self.gpu.is_none() {
                self.gpu = Self::device();
            }
            match freeze::freeze(self.gpu.as_ref()?, &monitors) {
                Ok(frozen) => {
                    tracing::debug!(
                        "{} screen(s) frozen in {:?}",
                        frozen.screens.len(),
                        started.elapsed()
                    );
                    return Some(frozen);
                }
                Err(e) => {
                    tracing::warn!("cannot freeze the screens: {e}");
                    freeze::thaw();
                    self.gpu = None;
                }
            }
        }
        None
    }

    fn thaw(&mut self) {
        freeze::thaw();
    }
}
