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
#[cfg(windows)]
mod wgc_stream;
#[cfg(windows)]
pub use wgc_stream::{CapturedFrame, GpuCapture, StreamTarget, TextureSink, VideoStream};

#[cfg(target_os = "macos")]
mod sck;
#[cfg(target_os = "macos")]
pub use sck::SckBackend;

pub use fake::FakeBackend;
pub use frame::{BYTES_PER_PIXEL, CpuFrame};
use vixeeny_platform::{HdrInfo, MonitorId, MonitorInfo, PhysicalRect, WindowId, virtual_bounds};

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

/// Converts scRGB floats (4 per pixel, `1.0` = 80 nits) to 8-bit BGRA; see
/// `vixeeny_image::tonemap`.
pub type ToneMapFn = fn(&[f32], &HdrInfo) -> Vec<u8>;

/// Plan 5.2: no cursor in still images by default.
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureOptions {
    pub show_cursor: bool,
    /// How to convert an HDR monitor to SDR. `None` captures HDR monitors as plain 8-bit.
    pub tonemap: Option<ToneMapFn>,
}

/// An HDR monitor frame: scRGB, linear BT.709 floats, 4 per pixel (R, G, B, A).
#[derive(Debug, Clone, PartialEq)]
pub struct HdrFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
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
    /// The whole monitor as scRGB floats. Only called for monitors with `hdr` set.
    fn grab_monitor_hdr(
        &mut self,
        _monitor: &MonitorInfo,
        _cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        Err(CaptureError::Unsupported)
    }
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

    /// One monitor, tone-mapped to SDR when it shows HDR and `options.tonemap` is set.
    fn monitor_frame(
        &mut self,
        monitor: &MonitorInfo,
        options: CaptureOptions,
    ) -> Result<CpuFrame, CaptureError> {
        if let (Some(info), Some(tonemap)) = (monitor.hdr, options.tonemap) {
            let hdr = self
                .backend
                .grab_monitor_hdr(monitor, options.show_cursor)?;
            let bgra = tonemap(&hdr.rgba, &info);
            let stride = hdr.width as usize * BYTES_PER_PIXEL;
            return CpuFrame::from_raw(hdr.width, hdr.height, stride, bgra);
        }
        self.backend.grab_monitor(monitor, options.show_cursor)
    }

    pub fn grab(
        &mut self,
        target: &CaptureTarget,
        options: CaptureOptions,
    ) -> Result<CpuFrame, CaptureError> {
        match target {
            CaptureTarget::Monitor(id) => {
                let monitor = self.monitor(*id)?.clone();
                self.monitor_frame(&monitor, options)
            }
            CaptureTarget::Region { monitor, rect } => {
                let monitor = self.monitor(*monitor)?.clone();
                let frame = self.monitor_frame(&monitor, options)?;
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
            let frame = self.monitor_frame(&monitor, options)?;
            let at = monitor.rect.relative_to(&bounds);
            canvas.blit(&frame, at.x, at.y);
        }
        Ok(canvas)
    }
}
