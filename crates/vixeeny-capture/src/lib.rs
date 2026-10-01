// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-capture — screen capture (plan 4.3). M4: single images (`grab`). The streaming side
//! (`start`/`stop` with a `FrameSink`) arrives with the video milestones.
//!
//! [`Capturer`] holds the target logic (regions, all monitors, DPI) and is tested with
//! [`FakeBackend`]; the OS only supplies whole-monitor and whole-window frames through
//! [`StillBackend`].

mod fake;
mod frame;

#[cfg(windows)]
mod wgc;
#[cfg(windows)]
pub use wgc::WgcBackend;

pub use fake::FakeBackend;
pub use frame::{BYTES_PER_PIXEL, CpuFrame};
use vixeeny_platform::{MonitorId, MonitorInfo, PhysicalRect, WindowId, virtual_bounds};

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureTarget {
    Monitor(MonitorId),
    /// `rect` is relative to the monitor's top-left corner, in physical pixels.
    Region {
        monitor: MonitorId,
        rect: PhysicalRect,
    },
    Window(WindowId),
    /// Every monitor, assembled on the virtual desktop canvas.
    AllMonitors,
}

/// Plan 5.2: no cursor in still images by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureOptions {
    pub show_cursor: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("unknown monitor")]
    UnknownMonitor,
    #[error("the region is empty or outside the monitor")]
    InvalidRegion,
    #[error("no monitor to capture")]
    NoMonitor,
    #[error("the capture produced no frame in time")]
    Timeout,
    #[error("not supported on this platform yet")]
    Unsupported,
    #[error("{0}")]
    Os(String),
}

/// What an OS must provide for single images.
pub trait StillBackend {
    /// The whole monitor, physical pixels.
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError>;
    /// The whole window content.
    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError>;
}

pub struct Capturer<B: StillBackend> {
    backend: B,
    monitors: Vec<MonitorInfo>,
}

impl<B: StillBackend> Capturer<B> {
    pub fn new(backend: B, monitors: Vec<MonitorInfo>) -> Self {
        Self { backend, monitors }
    }

    fn monitor(&self, id: MonitorId) -> Result<&MonitorInfo, CaptureError> {
        self.monitors
            .iter()
            .find(|m| m.id == id)
            .ok_or(CaptureError::UnknownMonitor)
    }

    pub fn grab(
        &mut self,
        target: &CaptureTarget,
        options: CaptureOptions,
    ) -> Result<CpuFrame, CaptureError> {
        match target {
            CaptureTarget::Monitor(id) => {
                let monitor = self.monitor(*id)?.clone();
                self.backend.grab_monitor(&monitor, options.show_cursor)
            }
            CaptureTarget::Region { monitor, rect } => {
                let monitor = self.monitor(*monitor)?.clone();
                let frame = self.backend.grab_monitor(&monitor, options.show_cursor)?;
                frame.crop(rect)
            }
            CaptureTarget::Window(id) => self.backend.grab_window(*id, options.show_cursor),
            CaptureTarget::AllMonitors => self.grab_all(options),
        }
    }

    fn grab_all(&mut self, options: CaptureOptions) -> Result<CpuFrame, CaptureError> {
        let bounds =
            virtual_bounds(self.monitors.iter().map(|m| &m.rect)).ok_or(CaptureError::NoMonitor)?;
        let mut canvas = CpuFrame::new(bounds.width, bounds.height);
        for monitor in self.monitors.clone() {
            let frame = self.backend.grab_monitor(&monitor, options.show_cursor)?;
            let at = monitor.rect.relative_to(&bounds);
            canvas.blit(&frame, at.x, at.y);
        }
        Ok(canvas)
    }
}
