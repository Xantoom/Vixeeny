// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-capture — screen capture: single images (`grab`). The streaming side
//! (`start`/`stop` with a `FrameSink`) arrives with the video milestones.
//!
//! [`Capturer`] holds the target logic (regions, all monitors, DPI) and is tested with
//! [`FakeBackend`]; the OS only supplies whole-monitor and whole-window frames through
//! [`StillBackend`].

mod fake;
mod frame;
pub mod freeze;
mod hdr;
mod wgc;
mod wgc_stream;
pub use fake::FakeBackend;
pub use frame::{BYTES_PER_PIXEL, CpuFrame};
pub use hdr::{HdrCapture, HdrFrame, reference};
use vixeeny_platform::{HdrInfo, MonitorId, MonitorInfo, PhysicalRect, WindowId, virtual_bounds};
pub use wgc::WgcBackend;
pub use wgc_stream::{CapturedFrame, GpuCapture, StreamTarget, TextureSink, VideoStream};

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
    /// `area` (relative to the monitor) limits it to a part, read alone: a zone captured 15
    /// times a second converts only its own pixels.
    fn grab_monitor_hdr(
        &mut self,
        _monitor: &MonitorInfo,
        _area: Option<&PhysicalRect>,
        _cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        Err(CaptureError::Unsupported)
    }
    /// The whole window content.
    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError>;
    /// The whole window content as scRGB floats. Only called for windows on an HDR monitor.
    fn grab_window_hdr(
        &mut self,
        _window: WindowId,
        _cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        Err(CaptureError::Unsupported)
    }
    /// The monitor showing most of the window.
    fn window_monitor(&self, _window: WindowId) -> Option<MonitorId> {
        None
    }
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
                .grab_monitor_hdr(monitor, None, options.show_cursor)?;
            return tonemapped(&hdr, &info, tonemap);
        }
        self.backend.grab_monitor(monitor, options.show_cursor)
    }

    /// The HDR monitor a window is on, if it is on one.
    fn window_hdr(&self, window: WindowId) -> Option<HdrInfo> {
        let id = self.backend.window_monitor(window)?;
        self.monitor(id).ok()?.hdr
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
                if let (Some(info), Some(tonemap)) = (monitor.hdr, options.tonemap) {
                    let hdr =
                        self.backend
                            .grab_monitor_hdr(&monitor, Some(rect), options.show_cursor)?;
                    return tonemapped(&hdr, &info, tonemap);
                }
                self.backend
                    .grab_monitor(&monitor, options.show_cursor)?
                    .crop(rect)
            }
            CaptureTarget::Window(id) => match (self.window_hdr(*id), options.tonemap) {
                (Some(info), Some(tonemap)) => {
                    let hdr = self.backend.grab_window_hdr(*id, options.show_cursor)?;
                    tonemapped(&hdr, &info, tonemap)
                }
                _ => self.backend.grab_window(*id, options.show_cursor),
            },
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

    /// The target as HDR, when it shows an HDR monitor; `None` when it is SDR only (take
    /// [`grab`](Self::grab) then). With several monitors, the SDR ones are placed at the SDR
    /// white of the HDR ones, and every monitor's SDR white is brought to the brightest.
    pub fn grab_hdr(
        &mut self,
        target: &CaptureTarget,
        cursor: bool,
    ) -> Result<Option<HdrCapture>, CaptureError> {
        match target {
            CaptureTarget::Monitor(id) | CaptureTarget::Region { monitor: id, .. } => {
                let monitor = self.monitor(*id)?.clone();
                let Some(info) = monitor.hdr else {
                    return Ok(None);
                };
                let area = match target {
                    CaptureTarget::Region { rect, .. } => Some(rect),
                    _ => None,
                };
                let frame = self.backend.grab_monitor_hdr(&monitor, area, cursor)?;
                Ok(Some(HdrCapture { frame, info }))
            }
            CaptureTarget::Window(id) => {
                let Some(info) = self.window_hdr(*id) else {
                    return Ok(None);
                };
                let frame = self.backend.grab_window_hdr(*id, cursor)?;
                Ok(Some(HdrCapture { frame, info }))
            }
            CaptureTarget::AllMonitors => self.grab_all_hdr(cursor),
        }
    }

    fn grab_all_hdr(&mut self, cursor: bool) -> Result<Option<HdrCapture>, CaptureError> {
        let Some(info) = reference(&self.monitors) else {
            return Ok(None);
        };
        let bounds =
            virtual_bounds(self.monitors.iter().map(|m| &m.rect)).ok_or(CaptureError::NoMonitor)?;
        let mut canvas = HdrFrame::new(bounds.width, bounds.height);
        for monitor in self.monitors.clone() {
            let frame = match monitor.hdr {
                Some(own) => {
                    let mut frame = self.backend.grab_monitor_hdr(&monitor, None, cursor)?;
                    frame.scale(info.sdr_white_nits / own.sdr_white_nits.max(1.0));
                    frame
                }
                None => HdrFrame::from_sdr(
                    &self.backend.grab_monitor(&monitor, cursor)?,
                    info.sdr_white_nits,
                ),
            };
            let at = monitor.rect.relative_to(&bounds);
            canvas.blit(&frame, at.x, at.y);
        }
        Ok(Some(HdrCapture {
            frame: canvas,
            info,
        }))
    }
}

fn tonemapped(
    hdr: &HdrFrame,
    info: &HdrInfo,
    tonemap: ToneMapFn,
) -> Result<CpuFrame, CaptureError> {
    let bgra = tonemap(&hdr.rgba, info);
    let stride = hdr.width as usize * BYTES_PER_PIXEL;
    CpuFrame::from_raw(hdr.width, hdr.height, stride, bgra)
}
