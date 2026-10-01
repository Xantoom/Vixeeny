// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows Graphics Capture, single-frame mode: create a capture session on a monitor or a
//! window, wait for the first frame, copy it to CPU memory and tear the session down.
//!
//! Own windows are kept out of monitor captures by `WDA_EXCLUDEFROMCAPTURE`, set by the
//! windows' owner (see `vixeeny_platform::exclude_from_capture`), not here.

use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use vixeeny_platform::{MonitorInfo, WindowId};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{Interface, factory};

use crate::{CaptureError, CpuFrame, HdrFrame, StillBackend};

const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

fn os<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> CaptureError + '_ {
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
        let mut device = None;
        let mut context = None;
        // SAFETY: out-pointers are valid; no feature-level list, software flags or adapter.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
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
        let dxgi: IDXGIDevice = device.cast().map_err(os("IDXGIDevice"))?;
        // SAFETY: `dxgi` is a live DXGI device.
        let winrt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .and_then(|inspectable| inspectable.cast())
            .map_err(os("CreateDirect3D11DeviceFromDXGIDevice"))?;
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
            self.read_back(&texture)
        })();
        let _ = pool.RemoveFrameArrived(token);
        let _ = session.Close();
        let _ = pool.Close();
        result
    }

    /// GPU → CPU through a staging texture.
    fn read_back(&self, texture: &ID3D11Texture2D) -> Result<RawFrame, CaptureError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a valid out-pointer.
        unsafe { texture.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.MiscFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        // SAFETY: `desc` describes a staging copy of an existing texture; the out-pointer is valid.
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&raw mut staging))
        }
        .map_err(os("CreateTexture2D"))?;
        let staging = staging.ok_or_else(|| CaptureError::Os("no staging texture".into()))?;
        // SAFETY: both textures belong to this device and have identical size and format.
        unsafe { self.context.CopyResource(&staging, texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture has CPU read access; `mapped` is a valid out-pointer.
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
        }
        .map_err(os("Map"))?;
        let (width, height) = (desc.Width, desc.Height);
        let stride = mapped.RowPitch as usize;
        // SAFETY: a mapped texture exposes `RowPitch * Height` readable bytes until `Unmap`.
        let bytes = unsafe {
            std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * height as usize)
        };
        let data = bytes.to_vec();
        // SAFETY: matches the successful Map above.
        unsafe { self.context.Unmap(&staging, 0) };
        Ok(RawFrame {
            width,
            height,
            stride,
            data,
        })
    }
}

/// Bytes of a mapped texture, rows `stride` apart.
struct RawFrame {
    width: u32,
    height: u32,
    stride: usize,
    data: Vec<u8>,
}

/// IEEE 754 half → single precision.
fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1F);
    let frac = f32::from(h & 0x3FF);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        0x1F if frac == 0.0 => sign * f32::INFINITY,
        0x1F => f32::NAN,
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

type FrameHandler = TypedEventHandler<Direct3D11CaptureFramePool, windows::core::IInspectable>;

fn frame_handler(tx: Sender<()>) -> FrameHandler {
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(());
        Ok(())
    })
}

impl StillBackend for WgcBackend {
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        let interop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(os("interop"))?;
        let hmonitor = HMONITOR(monitor.id.0 as usize as *mut _);
        // SAFETY: `hmonitor` came from `EnumDisplayMonitors`; a stale one makes the call fail.
        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForMonitor(hmonitor) }.map_err(os("CreateForMonitor"))?;
        let raw = self.grab_item(&item, cursor, DirectXPixelFormat::B8G8R8A8UIntNormalized)?;
        CpuFrame::from_raw(raw.width, raw.height, raw.stride, raw.data)
    }

    fn grab_monitor_hdr(
        &mut self,
        monitor: &MonitorInfo,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        let interop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(os("interop"))?;
        let hmonitor = HMONITOR(monitor.id.0 as usize as *mut _);
        // SAFETY: as in `grab_monitor`.
        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForMonitor(hmonitor) }.map_err(os("CreateForMonitor"))?;
        let raw = self.grab_item(&item, cursor, DirectXPixelFormat::R16G16B16A16Float)?;
        let row_bytes = raw.width as usize * 8;
        let mut rgba = Vec::with_capacity(raw.width as usize * raw.height as usize * 4);
        for y in 0..raw.height as usize {
            let row = &raw.data[y * raw.stride..y * raw.stride + row_bytes];
            rgba.extend(
                row.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|h| f16_to_f32(u16::from_le_bytes(*h))),
            );
        }
        Ok(HdrFrame {
            width: raw.width,
            height: raw.height,
            rgba,
        })
    }

    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError> {
        let interop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(os("interop"))?;
        let hwnd = HWND(window.0 as usize as *mut _);
        // SAFETY: a stale or invalid window handle makes the call fail.
        let item: GraphicsCaptureItem =
            unsafe { interop.CreateForWindow(hwnd) }.map_err(os("CreateForWindow"))?;
        let raw = self.grab_item(&item, cursor, DirectXPixelFormat::B8G8R8A8UIntNormalized)?;
        CpuFrame::from_raw(raw.width, raw.height, raw.stride, raw.data)
    }
}

#[cfg(test)]
mod tests {
    use super::f16_to_f32;

    #[test]
    fn half_floats() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
        assert!(f16_to_f32(0x7C00).is_infinite());
        assert!(f16_to_f32(0x7E00).is_nan());
        assert!(f16_to_f32(0x0001) > 0.0); // smallest subnormal
    }
}
