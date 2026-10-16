// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows Graphics Capture, single-frame mode: create a capture session on a monitor or a
//! window, wait for the first frame, copy it to CPU memory and tear the session down.
//!
//! Own windows are kept out of monitor captures by `WDA_EXCLUDEFROMCAPTURE`, set by the
//! windows' owner (see `vixeeny_overlay::popup`), not here.

use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use vixeeny_platform::{MonitorId, MonitorInfo, PhysicalRect, WindowId};
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
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONULL, MonitorFromWindow};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{Interface, factory};

use crate::{CaptureError, CpuFrame, HdrFrame, StillBackend};

pub(crate) const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn os<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> CaptureError + '_ {
    move |e| CaptureError::Os(format!("{what}: {e}"))
}

pub struct WgcBackend {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

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

    fn grab_item(
        &self,
        item: &GraphicsCaptureItem,
        cursor: bool,
        pixel_format: DirectXPixelFormat,
    ) -> Result<RawFrame, CaptureError> {
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
        // The cursor and border switches need recent Windows builds; ignore their absence.
        let _ = session.SetIsCursorCaptureEnabled(cursor);
        let _ = session.SetIsBorderRequired(false);
        let result = (|| {
            session.StartCapture().map_err(os("StartCapture"))?;
            rx.recv_timeout(FRAME_TIMEOUT)
                .map_err(|_| CaptureError::Timeout)?;
            let frame = pool.TryGetNextFrame().map_err(os("TryGetNextFrame"))?;
            let surface = frame.Surface().map_err(os("Surface"))?;
            let access: IDirect3DDxgiInterfaceAccess =
                surface.cast().map_err(os("surface access"))?;
            // SAFETY: the surface is backed by a D3D11 texture on our device.
            let texture: ID3D11Texture2D =
                unsafe { access.GetInterface() }.map_err(os("GetInterface"))?;
            read_back(&self.device, &self.context, &texture)
        })();
        let _ = pool.RemoveFrameArrived(token);
        let _ = session.Close();
        let _ = pool.Close();
        result
    }

    /// The item (or `area` of it) in scRGB: the half floats Windows composes HDR screens in.
    fn grab_hdr(
        &self,
        item: &GraphicsCaptureItem,
        area: Option<&PhysicalRect>,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        let raw = self.grab_item(item, cursor, DirectXPixelFormat::R16G16B16A16Float)?;
        let whole = PhysicalRect::new(0, 0, raw.width, raw.height);
        let area = area.copied().unwrap_or(whole);
        if area.is_empty() || whole.intersection(&area) != Some(area) {
            return Err(CaptureError::InvalidRegion);
        }
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
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: the staging texture has CPU read access; `mapped` is a valid out-pointer.
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }
        .map_err(os("Map"))?;
    let (width, height) = (desc.Width, desc.Height);
    let stride = mapped.RowPitch as usize;
    // SAFETY: a mapped texture exposes `RowPitch * Height` readable bytes until `Unmap`.
    let bytes =
        unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * height as usize) };
    let data = bytes.to_vec();
    // SAFETY: matches the successful Map above.
    unsafe { context.Unmap(&staging, 0) };
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
        let item = monitor_item(monitor)?;
        let raw = self.grab_item(&item, cursor, DirectXPixelFormat::B8G8R8A8UIntNormalized)?;
        CpuFrame::from_raw(raw.width, raw.height, raw.stride, raw.data)
    }

    fn grab_monitor_hdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        self.grab_hdr(&monitor_item(monitor)?, area, cursor)
    }

    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError> {
        let item = window_item(window)?;
        let raw = self.grab_item(&item, cursor, DirectXPixelFormat::B8G8R8A8UIntNormalized)?;
        CpuFrame::from_raw(raw.width, raw.height, raw.stride, raw.data)
    }

    fn grab_window_hdr(
        &mut self,
        window: WindowId,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        self.grab_hdr(&window_item(window)?, None, cursor)
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
        let one = 0x3C00u16.to_le_bytes();
        let two = 0x4000u16.to_le_bytes();
        let row = |h: [u8; 2]| [[h; 8].concat(), vec![0xFF; 4]].concat();
        let data = [row(one), row(two)].concat();
        let out = super::halves_to_f32(&data, 20, 2, 2);
        assert_eq!(out, [[1.0; 8], [2.0; 8]].concat());
    }
}
