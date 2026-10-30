// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen capture: still images of monitors, windows and regions, and the frame streams of recordings (`wgc_stream`).
//!
//! [`Capturer`] holds the target logic (regions, all monitors, DPI) and is tested with [`FakeBackend`]; the OS only
//! supplies whole-monitor and whole-window frames through [`StillBackend`].

mod card_tonemap;
mod dda;
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
pub use wgc_stream::{
    CapturedFrame, Gate, GpuCapture, HeldFrame, Notify, Pacing, StreamTarget, TextureSink, VideoStream,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

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

/// Converts scRGB floats (4 per pixel, `1.0` = 80 nits) to 8-bit BGRA; see `vixeeny_image::tonemap`.
pub type ToneMapFn = fn(&[f32], &HdrInfo) -> Vec<u8>;

/// No cursor in still images by default.
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
    #[error(transparent)]
    D3d(#[from] vixeeny_platform::d3d::D3dError),
}

/// A monitor as it shows: scRGB for an HDR one, 8-bit for the others.
pub enum Shown {
    Hdr(HdrFrame),
    Sdr(CpuFrame),
}

/// What the capture of still images needs from the system.
pub trait StillBackend {
    /// The whole monitor, physical pixels.
    fn grab_monitor(&mut self, monitor: &MonitorInfo, cursor: bool) -> Result<CpuFrame, CaptureError>;
    /// A region of an SDR monitor; GPU backends can read back only these pixels.
    fn grab_monitor_region(
        &mut self,
        monitor: &MonitorInfo,
        rect: &PhysicalRect,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        self.grab_monitor(monitor, cursor)?.crop(rect)
    }
    /// The whole monitor as scRGB floats. Only called for monitors with `hdr` set. `area` (relative to the monitor)
    /// limits it to a part, read alone: a zone captured 15 times a second converts only its own pixels.
    fn grab_monitor_hdr(
        &mut self,
        _monitor: &MonitorInfo,
        _area: Option<&PhysicalRect>,
        _cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        Err(CaptureError::Unsupported)
    }
    /// The monitor (or `area` of it) tone-mapped to SDR; only called for monitors with `hdr` set. A backend may do it
    /// on the graphics card, with the same tone mapper as `tonemap` and `tonemap` as its fallback.
    fn grab_monitor_sdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        cursor: bool,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        let hdr = self.grab_monitor_hdr(monitor, area, cursor)?;
        tonemapped(&hdr, info, tonemap)
    }
    /// The whole window content.
    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError>;
    /// The whole window content as scRGB floats. Only called for windows on an HDR monitor.
    fn grab_window_hdr(&mut self, _window: WindowId, _cursor: bool) -> Result<HdrFrame, CaptureError> {
        Err(CaptureError::Unsupported)
    }
    /// The whole window content tone-mapped to SDR, as [`grab_monitor_sdr`](Self::grab_monitor_sdr).
    fn grab_window_sdr(
        &mut self,
        window: WindowId,
        cursor: bool,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        let hdr = self.grab_window_hdr(window, cursor)?;
        tonemapped(&hdr, info, tonemap)
    }
    /// The monitor showing most of the window.
    fn window_monitor(&self, _window: WindowId) -> Option<MonitorId> {
        None
    }
    /// Every monitor of `monitors`, each as [`grab_monitor`](Self::grab_monitor) gives it, or
    /// [`grab_monitor_sdr`](Self::grab_monitor_sdr) for an HDR one when `tonemap` is set. A backend may capture them
    /// side by side.
    fn grab_monitors(
        &mut self,
        monitors: &[MonitorInfo],
        cursor: bool,
        tonemap: Option<ToneMapFn>,
    ) -> Result<Vec<CpuFrame>, CaptureError> {
        monitors
            .iter()
            .map(|m| match (m.hdr, tonemap) {
                (Some(info), Some(tonemap)) => self.grab_monitor_sdr(m, None, cursor, &info, tonemap),
                _ => self.grab_monitor(m, cursor),
            })
            .collect()
    }
    /// Every monitor of `monitors` as it shows: [`grab_monitor_hdr`](Self::grab_monitor_hdr) for an HDR one,
    /// [`grab_monitor`](Self::grab_monitor) for the others. A backend may capture them side by side.
    fn grab_monitors_shown(&mut self, monitors: &[MonitorInfo], cursor: bool) -> Result<Vec<Shown>, CaptureError> {
        monitors
            .iter()
            .map(|m| match m.hdr {
                Some(_) => self.grab_monitor_hdr(m, None, cursor).map(Shown::Hdr),
                None => self.grab_monitor(m, cursor).map(Shown::Sdr),
            })
            .collect()
    }
    /// Every monitor of `monitors` both tone-mapped to SDR (as [`grab_monitors`](Self::grab_monitors) gives it) and,
    /// for an HDR one, as scRGB too, both from the same frame. A backend may capture them side by side.
    fn grab_monitors_both(
        &mut self,
        monitors: &[MonitorInfo],
        cursor: bool,
        tonemap: ToneMapFn,
    ) -> Result<Vec<(CpuFrame, Option<HdrFrame>)>, CaptureError> {
        let shown = self.grab_monitors_shown(monitors, cursor)?;
        monitors
            .iter()
            .zip(shown)
            .map(|(m, shown)| match (shown, m.hdr) {
                (Shown::Hdr(hdr), Some(info)) => Ok((tonemapped(&hdr, &info, tonemap)?, Some(hdr))),
                (Shown::Hdr(_), None) => Err(CaptureError::Unsupported),
                (Shown::Sdr(sdr), _) => Ok((sdr, None)),
            })
            .collect()
    }
    /// The Direct3D device the captures are made on, for further work on the graphics card.
    fn d3d_device(&self) -> Option<ID3D11Device> {
        None
    }
}

/// A backend lent for a while (kept by its owner for the next captures).
impl<B: StillBackend + ?Sized> StillBackend for &mut B {
    fn grab_monitor(&mut self, monitor: &MonitorInfo, cursor: bool) -> Result<CpuFrame, CaptureError> {
        (**self).grab_monitor(monitor, cursor)
    }
    fn grab_monitor_hdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        (**self).grab_monitor_hdr(monitor, area, cursor)
    }
    fn grab_monitor_sdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        cursor: bool,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        (**self).grab_monitor_sdr(monitor, area, cursor, info, tonemap)
    }
    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError> {
        (**self).grab_window(window, cursor)
    }
    fn grab_window_hdr(&mut self, window: WindowId, cursor: bool) -> Result<HdrFrame, CaptureError> {
        (**self).grab_window_hdr(window, cursor)
    }
    fn grab_window_sdr(
        &mut self,
        window: WindowId,
        cursor: bool,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        (**self).grab_window_sdr(window, cursor, info, tonemap)
    }
    fn window_monitor(&self, window: WindowId) -> Option<MonitorId> {
        (**self).window_monitor(window)
    }
    fn grab_monitors(
        &mut self,
        monitors: &[MonitorInfo],
        cursor: bool,
        tonemap: Option<ToneMapFn>,
    ) -> Result<Vec<CpuFrame>, CaptureError> {
        (**self).grab_monitors(monitors, cursor, tonemap)
    }
    fn grab_monitors_shown(&mut self, monitors: &[MonitorInfo], cursor: bool) -> Result<Vec<Shown>, CaptureError> {
        (**self).grab_monitors_shown(monitors, cursor)
    }
    fn grab_monitors_both(
        &mut self,
        monitors: &[MonitorInfo],
        cursor: bool,
        tonemap: ToneMapFn,
    ) -> Result<Vec<(CpuFrame, Option<HdrFrame>)>, CaptureError> {
        (**self).grab_monitors_both(monitors, cursor, tonemap)
    }
    fn d3d_device(&self) -> Option<ID3D11Device> {
        (**self).d3d_device()
    }
}

/// Gives back the memory the driver keeps for the textures `device` freed (over 100 MB after a few 4K captures): the
/// next ones cost about a millisecond more to make. Call it once the device's work is done (its immediate context is
/// reset).
pub fn trim(device: &ID3D11Device) {
    use windows::Win32::Graphics::Dxgi::IDXGIDevice3;
    use windows::core::Interface;
    // SAFETY: plain calls on a live device; its immediate context is not in use.
    unsafe {
        if let Ok(context) = device.GetImmediateContext() {
            context.ClearState();
        }
        if let Ok(dxgi) = device.cast::<IDXGIDevice3>() {
            dxgi.Trim();
        }
    }
}

pub struct Capturer<B: StillBackend> {
    backend: B,
    monitors: Vec<MonitorInfo>,
}

impl<B: StillBackend> Capturer<B> {
    pub fn new(backend: B, monitors: Vec<MonitorInfo>) -> Self {
        if monitors.iter().any(|m| m.hdr.is_some()) {
            card_tonemap::warm();
        }
        Self { backend, monitors }
    }

    /// The backend's Direct3D device (see [`StillBackend::d3d_device`]).
    pub fn d3d_device(&self) -> Option<ID3D11Device> {
        self.backend.d3d_device()
    }

    fn monitor(&self, id: MonitorId) -> Result<&MonitorInfo, CaptureError> {
        self.monitors.iter().find(|m| m.id == id).ok_or(CaptureError::UnknownMonitor)
    }

    /// One monitor, tone-mapped to SDR when it shows HDR and `options.tonemap` is set.
    fn monitor_frame(&mut self, monitor: &MonitorInfo, options: CaptureOptions) -> Result<CpuFrame, CaptureError> {
        if let (Some(info), Some(tonemap)) = (monitor.hdr, options.tonemap) {
            return self.backend.grab_monitor_sdr(monitor, None, options.show_cursor, &info, tonemap);
        }
        self.backend.grab_monitor(monitor, options.show_cursor)
    }

    /// The HDR monitor a window is on, if it is on one.
    fn window_hdr(&self, window: WindowId) -> Option<HdrInfo> {
        let id = self.backend.window_monitor(window)?;
        self.monitor(id).ok()?.hdr
    }

    pub fn grab(&mut self, target: &CaptureTarget, options: CaptureOptions) -> Result<CpuFrame, CaptureError> {
        match target {
            CaptureTarget::Monitor(id) => {
                let monitor = self.monitor(*id)?.clone();
                self.monitor_frame(&monitor, options)
            }
            CaptureTarget::Region { monitor, rect } => {
                let monitor = self.monitor(*monitor)?.clone();
                if let (Some(info), Some(tonemap)) = (monitor.hdr, options.tonemap) {
                    return self.backend.grab_monitor_sdr(&monitor, Some(rect), options.show_cursor, &info, tonemap);
                }
                self.backend.grab_monitor_region(&monitor, rect, options.show_cursor)
            }
            CaptureTarget::Window(id) => match (self.window_hdr(*id), options.tonemap) {
                (Some(info), Some(tonemap)) => self.backend.grab_window_sdr(*id, options.show_cursor, &info, tonemap),
                _ => self.backend.grab_window(*id, options.show_cursor),
            },
            CaptureTarget::AllMonitors => self.grab_all(options),
        }
    }

    fn grab_all(&mut self, options: CaptureOptions) -> Result<CpuFrame, CaptureError> {
        let bounds = virtual_bounds(self.monitors.iter().map(|m| &m.rect)).ok_or(CaptureError::NoMonitor)?;
        let mut canvas = CpuFrame::new(bounds.width, bounds.height);
        let frames = self.backend.grab_monitors(&self.monitors, options.show_cursor, options.tonemap)?;
        for (monitor, frame) in self.monitors.iter().zip(frames) {
            let at = monitor.rect.relative_to(&bounds);
            canvas.blit(&frame, at.x, at.y);
        }
        Ok(canvas)
    }

    /// The target as HDR, when it shows an HDR monitor; `None` when it is SDR only (take [`grab`](Self::grab) then).
    /// With several monitors, the SDR ones are placed at the SDR white of the HDR ones, and every monitor's SDR white
    /// is brought to the brightest.
    pub fn grab_hdr(&mut self, target: &CaptureTarget, cursor: bool) -> Result<Option<HdrCapture>, CaptureError> {
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

    /// Every monitor as [`grab`](Self::grab) (tone-mapped with `tonemap`) and [`grab_hdr`](Self::grab_hdr) give them
    /// for [`CaptureTarget::AllMonitors`], from one capture: both show the same moment.
    pub fn grab_all_both(
        &mut self,
        cursor: bool,
        tonemap: ToneMapFn,
    ) -> Result<(CpuFrame, Option<HdrCapture>), CaptureError> {
        let options = CaptureOptions { show_cursor: cursor, tonemap: Some(tonemap) };
        let Some(info) = reference(&self.monitors) else {
            return Ok((self.grab_all(options)?, None));
        };
        let bounds = virtual_bounds(self.monitors.iter().map(|m| &m.rect)).ok_or(CaptureError::NoMonitor)?;
        let mut sdr = CpuFrame::new(bounds.width, bounds.height);
        let mut hdr = HdrFrame::new(bounds.width, bounds.height);
        let frames = self.backend.grab_monitors_both(&self.monitors, cursor, tonemap)?;
        for (monitor, (toned, scrgb)) in self.monitors.iter().zip(frames) {
            let at = monitor.rect.relative_to(&bounds);
            sdr.blit(&toned, at.x, at.y);
            let (x, y) = (at.x as u32, at.y as u32);
            match scrgb {
                Some(mut frame) => {
                    let own = monitor.hdr.map_or(info.sdr_white_nits, |o| o.sdr_white_nits);
                    frame.scale(info.sdr_white_nits / own.max(1.0));
                    hdr.blit(&frame, at.x, at.y);
                }
                None => {
                    let size = (toned.width, toned.height);
                    hdr.put_bgra(x, y, size, toned.stride, &toned.data, info.sdr_white_nits);
                }
            }
        }
        Ok((sdr, Some(HdrCapture { frame: hdr, info })))
    }

    fn grab_all_hdr(&mut self, cursor: bool) -> Result<Option<HdrCapture>, CaptureError> {
        let Some(info) = reference(&self.monitors) else {
            return Ok(None);
        };
        let bounds = virtual_bounds(self.monitors.iter().map(|m| &m.rect)).ok_or(CaptureError::NoMonitor)?;
        let mut canvas = HdrFrame::new(bounds.width, bounds.height);
        let shown = self.backend.grab_monitors_shown(&self.monitors, cursor)?;
        for (monitor, shown) in self.monitors.iter().zip(shown) {
            let at = monitor.rect.relative_to(&bounds);
            match (shown, monitor.hdr) {
                (Shown::Hdr(mut frame), own) => {
                    let own = own.map_or(info.sdr_white_nits, |o| o.sdr_white_nits);
                    frame.scale(info.sdr_white_nits / own.max(1.0));
                    canvas.blit(&frame, at.x, at.y);
                }
                (Shown::Sdr(frame), _) => {
                    let (x, y, size) = (at.x as u32, at.y as u32, (frame.width, frame.height));
                    canvas.put_bgra(x, y, size, frame.stride, &frame.data, info.sdr_white_nits);
                }
            }
        }
        Ok(Some(HdrCapture { frame: canvas, info }))
    }
}

pub(crate) fn tonemapped(hdr: &HdrFrame, info: &HdrInfo, tonemap: ToneMapFn) -> Result<CpuFrame, CaptureError> {
    let bgra = tonemap(&hdr.rgba, info);
    let stride = hdr.width as usize * BYTES_PER_PIXEL;
    CpuFrame::from_raw(hdr.width, hdr.height, stride, bgra)
}
