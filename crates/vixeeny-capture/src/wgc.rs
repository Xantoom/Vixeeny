// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows Graphics Capture, single-frame mode: create a capture session on a monitor or a
//! window, wait for the first frame, copy it to CPU memory and tear the session down. A monitor
//! is taken through Desktop Duplication first, which gives it without the wait (see
//! [`crate::dda`]).
//!
//! Own windows are kept out of monitor captures by `WDA_EXCLUDEFROMCAPTURE`, set by the
//! windows' owner (see `vixeeny_overlay::popup`), not here.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use vixeeny_platform::{HdrInfo, MonitorId, MonitorInfo, PhysicalRect, WindowId};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION, D3D11_SDK_VERSION,
    D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONULL, MonitorFromWindow};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{Interface, factory};

use crate::{
    CaptureError, CpuFrame, HdrFrame, Shown, StillBackend, ToneMapFn, card_tonemap, tonemapped,
};

pub(crate) const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn os<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> CaptureError + '_ {
    move |e| CaptureError::Os(format!("{what}: {e}"))
}

pub struct WgcBackend {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

// SAFETY: a Direct3D 11 device is free-threaded and its WinRT wrapper is agile; the immediate
// context is not thread-safe, but a moved backend is used by one thread at a time (it is not
// `Sync`). It is made on one thread and used on another (the app makes it while starting).
unsafe impl Send for WgcBackend {}

impl WgcBackend {
    pub fn new() -> Result<Self, CaptureError> {
        if !GraphicsCaptureSession::IsSupported().map_err(os("IsSupported"))? {
            return Err(CaptureError::Os(
                "Windows Graphics Capture is not available".into(),
            ));
        }
        let (device, context) = d3d_device(None)?;
        let winrt_device = winrt_device(&device)?;
        Ok(Self {
            device,
            context,
            winrt_device,
        })
    }

    /// Every monitor (none of them HDR), assembled on the graphics card into one picture of the
    /// virtual desktop and read back once: `read` gets its size and its BGRA rows, `stride`
    /// bytes apart, while they are mapped. No pointer. [`CaptureError::Unsupported`] with an HDR
    /// monitor or a desktop too large for one texture.
    pub fn grab_desktop<T>(
        &mut self,
        monitors: &[MonitorInfo],
        read: impl FnOnce(u32, u32, &[u8], usize) -> T,
    ) -> Result<T, CaptureError> {
        let bounds = vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect))
            .ok_or(CaptureError::NoMonitor)?;
        if monitors.iter().any(|m| m.hdr.is_some())
            || bounds.width.max(bounds.height) > D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION
        {
            return Err(CaptureError::Unsupported);
        }
        let format = DirectXPixelFormat::B8G8R8A8UIntNormalized;
        // Every capture starts before any is waited for: they run side by side.
        let started = monitors
            .iter()
            .map(|m| self.start_monitor(m, false, format))
            .collect::<Result<Vec<_>, _>>()?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: bounds.width,
            Height: bounds.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        // What no monitor covers is black, as on an assembled frame.
        let covered: u64 = monitors.iter().map(|m| m.rect.area()).sum();
        let black = (covered < bounds.area())
            .then(|| vec![0u8; bounds.width as usize * bounds.height as usize * 4]);
        let init = black.as_ref().map(|b| D3D11_SUBRESOURCE_DATA {
            pSysMem: b.as_ptr().cast(),
            SysMemPitch: bounds.width * 4,
            SysMemSlicePitch: 0,
        });
        let mut staging = None;
        // SAFETY: `desc` describes a staging texture; `init` points at `black`, which outlives
        // the call; the out-pointer is valid.
        unsafe {
            self.device.CreateTexture2D(
                &desc,
                init.as_ref().map(|i| &raw const *i),
                Some(&raw mut staging),
            )
        }
        .map_err(os("CreateTexture2D"))?;
        drop(black);
        let staging = staging.ok_or_else(|| CaptureError::Os("no staging texture".into()))?;
        for (m, started) in monitors.iter().zip(started) {
            let at = m.rect.relative_to(&bounds);
            started.finish(|texture| {
                // SAFETY: both textures belong to this device, in the same format; the monitor's
                // picture lies inside the desktop's.
                unsafe {
                    self.context.CopySubresourceRegion(
                        &staging,
                        0,
                        at.x as u32,
                        at.y as u32,
                        0,
                        texture,
                        0,
                        None,
                    );
                }
                Ok(())
            })?;
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture has CPU read access; `mapped` is a valid out-pointer.
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
        }
        .map_err(os("Map"))?;
        let stride = mapped.RowPitch as usize;
        // SAFETY: a mapped texture exposes `RowPitch * Height` readable bytes until `Unmap`.
        let rows = unsafe {
            std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * bounds.height as usize)
        };
        let result = read(bounds.width, bounds.height, rows, stride);
        // SAFETY: matches the successful Map above.
        unsafe { self.context.Unmap(&staging, 0) };
        Ok(result)
    }

    /// Whether the device is gone (a driver update or reset): a new backend is needed.
    pub fn is_lost(&self) -> bool {
        // SAFETY: a plain query of a live device.
        unsafe { self.device.GetDeviceRemovedReason() }.is_err()
    }

    /// The item as plain 8-bit BGRA.
    fn grab_sdr_item(
        &self,
        item: &GraphicsCaptureItem,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        let format = DirectXPixelFormat::B8G8R8A8UIntNormalized;
        self.grab_item(item, cursor, format, |texture| self.read_sdr(texture))
    }

    /// A frame of 8-bit BGRA.
    fn read_sdr(&self, texture: &ID3D11Texture2D) -> Result<CpuFrame, CaptureError> {
        let raw = read_back(&self.device, &self.context, texture)?;
        CpuFrame::from_raw(raw.width, raw.height, raw.stride, raw.data)
    }

    /// The first frame of `item`, handed to `read` while it is held.
    fn grab_item<T>(
        &self,
        item: &GraphicsCaptureItem,
        cursor: bool,
        pixel_format: DirectXPixelFormat,
        read: impl FnOnce(&ID3D11Texture2D) -> Result<T, CaptureError>,
    ) -> Result<T, CaptureError> {
        self.start(item, cursor, pixel_format)?.finish(read)
    }

    /// `monitor`'s picture in `pixel_format`: duplicated when it can be (not with the pointer),
    /// captured otherwise.
    fn start_monitor(
        &self,
        monitor: &MonitorInfo,
        cursor: bool,
        pixel_format: DirectXPixelFormat,
    ) -> Result<Grab, CaptureError> {
        let hdr = pixel_format == DirectXPixelFormat::R16G16B16A16Float;
        if !cursor && let Some(frame) = crate::dda::frame(&self.device, monitor, hdr) {
            return Ok(Grab::Duplicated(frame));
        }
        let item = monitor_item(monitor)?;
        Ok(Grab::Captured(self.start(&item, cursor, pixel_format)?))
    }

    /// A capture of `item` under way: its first frame is waited for by [`Started::finish`].
    fn start(
        &self,
        item: &GraphicsCaptureItem,
        cursor: bool,
        pixel_format: DirectXPixelFormat,
    ) -> Result<Started, CaptureError> {
        let size = item.Size().map_err(os("item size"))?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &self.winrt_device,
            pixel_format,
            1,
            size,
        )
        .map_err(os("CreateFreeThreaded"))?;
        let (tx, rx) = channel::<()>();
        let handler = frame_handler(tx);
        let token = pool.FrameArrived(&handler).map_err(os("FrameArrived"))?;
        let session = pool
            .CreateCaptureSession(item)
            .map_err(os("CreateCaptureSession"))?;
        let started = Started {
            pool,
            session,
            token,
            arrived: rx,
        };
        // The cursor and border switches need recent Windows builds; ignore their absence.
        let _ = started.session.SetIsCursorCaptureEnabled(cursor);
        let _ = started.session.SetIsBorderRequired(false);
        started.session.StartCapture().map_err(os("StartCapture"))?;
        Ok(started)
    }

    /// The item (or `area` of it) in scRGB: the half floats Windows composes HDR screens in.
    fn grab_hdr(
        &self,
        item: &GraphicsCaptureItem,
        area: Option<&PhysicalRect>,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        self.grab_item(
            item,
            cursor,
            DirectXPixelFormat::R16G16B16A16Float,
            |texture| {
                let raw = read_back(&self.device, &self.context, texture)?;
                raw_hdr(&raw, area)
            },
        )
    }

    /// The item (or `area` of it) tone-mapped to SDR on the graphics card, or with `tonemap`
    /// when the card cannot.
    fn grab_sdr(
        &self,
        item: &GraphicsCaptureItem,
        area: Option<&PhysicalRect>,
        cursor: bool,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        self.grab_item(
            item,
            cursor,
            DirectXPixelFormat::R16G16B16A16Float,
            |texture| self.read_tonemapped(texture, area, info, tonemap),
        )
    }

    /// `area` of a frame of half floats, tone-mapped (see [`grab_sdr`](Self::grab_sdr)).
    fn read_tonemapped(
        &self,
        texture: &ID3D11Texture2D,
        area: Option<&PhysicalRect>,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a valid out-pointer.
        unsafe { texture.GetDesc(&mut desc) };
        let whole = PhysicalRect::new(0, 0, desc.Width, desc.Height);
        let part = inside(area, whole)?;
        let card = card_tonemap::stage(&self.device, &self.context, texture, &part, info)
            .and_then(|staging| map(&self.context, &staging, part.width, part.height));
        match card {
            Ok(raw) => CpuFrame::from_raw(raw.width, raw.height, raw.stride, raw.data),
            Err(_) => {
                let raw = read_back(&self.device, &self.context, texture)?;
                tonemapped(&raw_hdr(&raw, area)?, info, tonemap)
            }
        }
    }
}

/// A monitor's picture on its way: duplicated already, or captured once its frame comes.
enum Grab {
    Duplicated(crate::dda::Frame),
    Captured(Started),
}

impl Grab {
    /// The picture, handed to `read` while it is held.
    fn finish<T>(
        self,
        read: impl FnOnce(&ID3D11Texture2D) -> Result<T, CaptureError>,
    ) -> Result<T, CaptureError> {
        match self {
            Self::Duplicated(frame) => read(&frame.texture),
            Self::Captured(started) => started.finish(read),
        }
    }
}

/// A capture session waiting for its first frame; closed when dropped.
struct Started {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: i64,
    arrived: Receiver<()>,
}

impl Started {
    /// The first frame, handed to `read` while it is held.
    fn finish<T>(
        self,
        read: impl FnOnce(&ID3D11Texture2D) -> Result<T, CaptureError>,
    ) -> Result<T, CaptureError> {
        self.arrived
            .recv_timeout(FRAME_TIMEOUT)
            .map_err(|_| CaptureError::Timeout)?;
        let frame = self.pool.TryGetNextFrame().map_err(os("TryGetNextFrame"))?;
        let surface = frame.Surface().map_err(os("Surface"))?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(os("surface access"))?;
        // SAFETY: the surface is backed by a D3D11 texture on our device.
        let texture: ID3D11Texture2D =
            unsafe { access.GetInterface() }.map_err(os("GetInterface"))?;
        read(&texture)
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// `area` of a frame of `whole`, which it must lie in; all of it when `None`.
fn inside(area: Option<&PhysicalRect>, whole: PhysicalRect) -> Result<PhysicalRect, CaptureError> {
    let area = area.copied().unwrap_or(whole);
    if area.is_empty() || whole.intersection(&area) != Some(area) {
        return Err(CaptureError::InvalidRegion);
    }
    Ok(area)
}

/// `area` of RGBA half floats as scRGB.
fn raw_hdr(raw: &RawFrame, area: Option<&PhysicalRect>) -> Result<HdrFrame, CaptureError> {
    let area = inside(area, PhysicalRect::new(0, 0, raw.width, raw.height))?;
    let start = area.y as usize * raw.stride + area.x as usize * 8;
    let rgba = halves_to_f32(
        &raw.data[start..],
        raw.stride,
        area.width as usize,
        area.height as usize,
    );
    Ok(HdrFrame {
        width: area.width,
        height: area.height,
        rgba,
    })
}

/// GPU → CPU through a staging texture.
pub(crate) fn read_back(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
) -> Result<RawFrame, CaptureError> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.MiscFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    // SAFETY: `desc` describes a staging copy of an existing texture; the out-pointer is valid.
    unsafe { device.CreateTexture2D(&desc, None, Some(&raw mut staging)) }
        .map_err(os("CreateTexture2D"))?;
    let staging = staging.ok_or_else(|| CaptureError::Os("no staging texture".into()))?;
    // SAFETY: both textures belong to this device and have identical size and format.
    unsafe { context.CopyResource(&staging, texture) };
    map(context, &staging, desc.Width, desc.Height)
}

/// The bytes of a staging texture of `width` × `height`, once the GPU has written them.
pub(crate) fn map(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<RawFrame, CaptureError> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: the staging texture has CPU read access; `mapped` is a valid out-pointer.
    unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }
        .map_err(os("Map"))?;
    let stride = mapped.RowPitch as usize;
    // SAFETY: a mapped texture exposes `RowPitch * Height` readable bytes until `Unmap`.
    let bytes =
        unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * height as usize) };
    let data = bytes.to_vec();
    // SAFETY: matches the successful Map above.
    unsafe { context.Unmap(staging, 0) };
    Ok(RawFrame {
        width,
        height,
        stride,
        data,
    })
}

/// Bytes of a mapped texture, rows `stride` apart.
pub(crate) struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
}

/// Rows of RGBA half floats (`stride` bytes apart) → tightly packed RGBA floats, over every core.
pub(crate) fn halves_to_f32(data: &[u8], stride: usize, width: usize, height: usize) -> Vec<f32> {
    use rayon::prelude::*;

    let mut out = vec![0.0; width * height * 4];
    out.par_chunks_mut(width * 4)
        .zip(data.par_chunks(stride))
        .for_each(|(out, row)| {
            for (o, h) in out.iter_mut().zip(row[..width * 8].as_chunks::<2>().0) {
                *o = vixeeny_common::half::to_f32(u16::from_le_bytes(*h));
            }
        });
    out
}

type FrameHandler = TypedEventHandler<Direct3D11CaptureFramePool, windows::core::IInspectable>;

pub(crate) fn frame_handler(tx: Sender<()>) -> FrameHandler {
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(());
        Ok(())
    })
}

/// A hardware Direct3D 11 device, on `adapter` or the default one.
pub(crate) fn d3d_device(
    adapter: Option<&IDXGIAdapter>,
) -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    let mut device = None;
    let mut context = None;
    // An explicit adapter needs the "unknown" driver type.
    let driver = if adapter.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };
    // SAFETY: out-pointers are valid; no feature-level list or software module.
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .map_err(os("D3D11CreateDevice"))?;
    let device = device.ok_or_else(|| CaptureError::Os("no D3D11 device".into()))?;
    let context = context.ok_or_else(|| CaptureError::Os("no D3D11 context".into()))?;
    Ok((device, context))
}

/// The WinRT wrapper Windows Graphics Capture wants.
pub(crate) fn winrt_device(device: &ID3D11Device) -> Result<IDirect3DDevice, CaptureError> {
    let dxgi: IDXGIDevice = device.cast().map_err(os("IDXGIDevice"))?;
    // SAFETY: `dxgi` is a live DXGI device.
    unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .and_then(|inspectable| inspectable.cast())
        .map_err(os("CreateDirect3D11DeviceFromDXGIDevice"))
}

/// The capture item of a monitor.
pub(crate) fn monitor_item(monitor: &MonitorInfo) -> Result<GraphicsCaptureItem, CaptureError> {
    let interop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(os("interop"))?;
    let hmonitor = HMONITOR(monitor.id.0 as usize as *mut _);
    // SAFETY: `hmonitor` came from `EnumDisplayMonitors`; a stale one makes the call fail.
    unsafe { interop.CreateForMonitor(hmonitor) }.map_err(os("CreateForMonitor"))
}

impl StillBackend for WgcBackend {
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        self.start_monitor(monitor, cursor, DirectXPixelFormat::B8G8R8A8UIntNormalized)?
            .finish(|t| self.read_sdr(t))
    }

    fn grab_monitor_hdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        self.start_monitor(monitor, cursor, DirectXPixelFormat::R16G16B16A16Float)?
            .finish(|t| {
                let raw = read_back(&self.device, &self.context, t)?;
                raw_hdr(&raw, area)
            })
    }

    fn grab_monitor_sdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        cursor: bool,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        self.start_monitor(monitor, cursor, DirectXPixelFormat::R16G16B16A16Float)?
            .finish(|t| self.read_tonemapped(t, area, info, tonemap))
    }

    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError> {
        self.grab_sdr_item(&window_item(window)?, cursor)
    }

    fn grab_window_hdr(
        &mut self,
        window: WindowId,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        self.grab_hdr(&window_item(window)?, None, cursor)
    }

    fn grab_window_sdr(
        &mut self,
        window: WindowId,
        cursor: bool,
        info: &HdrInfo,
        tonemap: ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        self.grab_sdr(&window_item(window)?, None, cursor, info, tonemap)
    }

    fn grab_monitors(
        &mut self,
        monitors: &[MonitorInfo],
        cursor: bool,
        tonemap: Option<ToneMapFn>,
    ) -> Result<Vec<CpuFrame>, CaptureError> {
        // Every capture starts before any is waited for: they run side by side.
        let started = monitors
            .iter()
            .map(|m| {
                let format = match (m.hdr, tonemap) {
                    (Some(_), Some(_)) => DirectXPixelFormat::R16G16B16A16Float,
                    _ => DirectXPixelFormat::B8G8R8A8UIntNormalized,
                };
                self.start_monitor(m, cursor, format)
            })
            .collect::<Result<Vec<_>, _>>()?;
        monitors
            .iter()
            .zip(started)
            .map(|(m, started)| match (m.hdr, tonemap) {
                (Some(info), Some(tonemap)) => {
                    started.finish(|t| self.read_tonemapped(t, None, &info, tonemap))
                }
                _ => started.finish(|t| self.read_sdr(t)),
            })
            .collect()
    }

    fn grab_monitors_shown(
        &mut self,
        monitors: &[MonitorInfo],
        cursor: bool,
    ) -> Result<Vec<Shown>, CaptureError> {
        // Every capture starts before any is waited for: they run side by side.
        let started = monitors
            .iter()
            .map(|m| {
                let format = match m.hdr {
                    Some(_) => DirectXPixelFormat::R16G16B16A16Float,
                    None => DirectXPixelFormat::B8G8R8A8UIntNormalized,
                };
                self.start_monitor(m, cursor, format)
            })
            .collect::<Result<Vec<_>, _>>()?;
        monitors
            .iter()
            .zip(started)
            .map(|(m, started)| match m.hdr {
                Some(_) => started.finish(|t| {
                    let raw = read_back(&self.device, &self.context, t)?;
                    raw_hdr(&raw, None).map(Shown::Hdr)
                }),
                None => started.finish(|t| self.read_sdr(t).map(Shown::Sdr)),
            })
            .collect()
    }

    fn grab_monitors_both(
        &mut self,
        monitors: &[MonitorInfo],
        cursor: bool,
        tonemap: ToneMapFn,
    ) -> Result<Vec<(CpuFrame, Option<HdrFrame>)>, CaptureError> {
        // Every capture starts before any is waited for: they run side by side.
        let started = monitors
            .iter()
            .map(|m| {
                let format = match m.hdr {
                    Some(_) => DirectXPixelFormat::R16G16B16A16Float,
                    None => DirectXPixelFormat::B8G8R8A8UIntNormalized,
                };
                self.start_monitor(m, cursor, format)
            })
            .collect::<Result<Vec<_>, _>>()?;
        monitors
            .iter()
            .zip(started)
            .map(|(m, started)| match m.hdr {
                Some(info) => started.finish(|t| {
                    let toned = self.read_tonemapped(t, None, &info, tonemap)?;
                    let raw = read_back(&self.device, &self.context, t)?;
                    Ok((toned, Some(raw_hdr(&raw, None)?)))
                }),
                None => started.finish(|t| Ok((self.read_sdr(t)?, None))),
            })
            .collect()
    }

    fn d3d_device(&self) -> Option<ID3D11Device> {
        Some(self.device.clone())
    }

    fn window_monitor(&self, window: WindowId) -> Option<MonitorId> {
        // SAFETY: a stale window handle gives a null monitor.
        let monitor = unsafe { MonitorFromWindow(hwnd(window), MONITOR_DEFAULTTONULL) };
        (!monitor.is_invalid()).then_some(MonitorId(monitor.0 as usize as u64))
    }
}

fn hwnd(window: WindowId) -> HWND {
    HWND(window.0 as usize as *mut _)
}

/// The capture item of a window.
fn window_item(window: WindowId) -> Result<GraphicsCaptureItem, CaptureError> {
    let interop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(os("interop"))?;
    // SAFETY: a stale or invalid window handle makes the call fail.
    unsafe { interop.CreateForWindow(hwnd(window)) }.map_err(os("CreateForWindow"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn padded_rows_of_halves() {
        // Two pixels per row, a 4-byte row padding.
        let one = 0x3c00u16.to_le_bytes();
        let two = 0x4000u16.to_le_bytes();
        let row = |h: [u8; 2]| [[h; 8].concat(), vec![0xff; 4]].concat();
        let data = [row(one), row(two)].concat();
        let out = super::halves_to_f32(&data, 20, 2, 2);
        assert_eq!(out, [[1.0; 8], [2.0; 8]].concat());
    }

    /// The desktop assembled on the graphics card is the one assembled from each monitor.
    #[test]
    fn the_desktop_is_assembled_as_its_monitors() {
        use crate::{CaptureOptions, CaptureTarget, Capturer};
        vixeeny_platform::ensure_dpi_aware();
        let (Ok(mut backend), Ok(monitors)) =
            (super::WgcBackend::new(), vixeeny_platform::monitors())
        else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let started = std::time::Instant::now();
        let desktop = match backend.grab_desktop(&monitors, |w, h, rows, stride| {
            (w, h, stride, rows.to_vec())
        }) {
            Ok(desktop) => desktop,
            Err(crate::CaptureError::Unsupported) => {
                vixeeny_common::skip_test("an HDR monitor");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        eprintln!("desktop in {:?}", started.elapsed());
        let (w, h, stride, rows) = desktop;
        let assembled = Capturer::new(&mut backend, monitors.clone())
            .grab(&CaptureTarget::AllMonitors, CaptureOptions::default())
            .unwrap();
        assert_eq!((w, h), (assembled.width, assembled.height));
        // Colours only, most of them: anything animated moves between the two.
        let (mut same, mut n) = (0u64, 0u64);
        for y in (0..h as usize).step_by(7) {
            for x in (0..w as usize * 4).step_by(13).filter(|x| x % 4 != 3) {
                let (a, b) = (
                    rows[y * stride + x],
                    assembled.data[y * assembled.stride + x],
                );
                same += u64::from(a.abs_diff(b) <= 2);
                n += 1;
            }
        }
        assert!(same * 2 > n, "{same} of {n} samples the same");
    }
}
