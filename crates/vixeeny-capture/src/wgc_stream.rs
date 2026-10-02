// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows Graphics Capture, streaming mode (plan 5.9): a capture session that stays open and
//! delivers every frame with the OS monotonic timestamp (`SystemRelativeTime`, the master clock
//! of the recording pipeline) as BGRA in RAM. WGC only produces a frame when the content
//! changes: the consumer repeats the last one to keep a constant frame rate.
//!
//! This is the CPU path (GPU → staging texture → RAM). The zero-copy GPU path for hardware
//! encoders builds on the same session.

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vixeeny_platform::{MonitorInfo, PhysicalRect, WindowId};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface, factory};

use crate::{CaptureError, CpuFrame};

/// Frames the consumer may be behind by before new ones are dropped.
const BACKLOG: usize = 3;

fn os<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> CaptureError + '_ {
    move |e| CaptureError::Os(format!("{what}: {e}"))
}

fn pixel_format(hdr: bool) -> DirectXPixelFormat {
    if hdr {
        DirectXPixelFormat::R16G16B16A16Float
    } else {
        DirectXPixelFormat::B8G8R8A8UIntNormalized
    }
}

/// What to follow.
#[derive(Debug, Clone)]
pub enum StreamTarget {
    Monitor(MonitorInfo),
    /// The whole window; follows it when it moves or is resized.
    Window(WindowId),
    /// A part of a monitor, in physical pixels relative to the monitor.
    Region {
        monitor: MonitorInfo,
        rect: PhysicalRect,
    },
}

/// A frame and when it was produced.
#[derive(Debug)]
pub struct CapturedFrame {
    /// `true`: `frame.data` holds RGBA half floats (scRGB, 8 bytes per pixel) instead of BGRA8.
    pub hdr: bool,
    /// Nanoseconds on the OS monotonic clock (the same clock as
    /// `vixeeny_platform::monotonic_ns`).
    pub time_ns: i64,
    pub frame: CpuFrame,
}

/// State shared with the frame callback. The immediate context is not thread safe: one lock.
struct Shared {
    device: ID3D11Device,
    winrt_device: IDirect3DDevice,
    context: Mutex<ID3D11DeviceContext>,
    staging: Mutex<Option<(ID3D11Texture2D, (u32, u32))>>,
    region: Option<PhysicalRect>,
    hdr: bool,
    last_size: Mutex<SizeInt32>,
}

// SAFETY: a D3D11 device is free-threaded; the immediate context and the staging texture are
// only used under their mutexes; the other fields are WinRT objects (agile) or plain data.
unsafe impl Send for Shared {}
// SAFETY: see above.
unsafe impl Sync for Shared {}

impl Shared {
    /// Copies `texture` (its `content` part, cropped to the region) into a CPU frame.
    fn read(
        &self,
        texture: &ID3D11Texture2D,
        content: SizeInt32,
    ) -> Result<CpuFrame, CaptureError> {
        let (cw, ch) = (content.Width.max(1) as u32, content.Height.max(1) as u32);
        let (x, y, w, h) = match self.region {
            Some(r) => {
                let x = r.x.max(0) as u32;
                let y = r.y.max(0) as u32;
                (
                    x,
                    y,
                    r.width.min(cw.saturating_sub(x)),
                    r.height.min(ch.saturating_sub(y)),
                )
            }
            None => (0, 0, cw, ch),
        };
        if w == 0 || h == 0 {
            return Err(CaptureError::InvalidRegion);
        }
        let context = self
            .context
            .lock()
            .map_err(|_| CaptureError::Os("capture lock poisoned".into()))?;
        let mut cache = self
            .staging
            .lock()
            .map_err(|_| CaptureError::Os("capture lock poisoned".into()))?;
        if cache.as_ref().is_none_or(|(_, size)| *size != (w, h)) {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `desc` is a valid out-pointer.
            unsafe { texture.GetDesc(&mut desc) };
            desc.Width = w;
            desc.Height = h;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            // SAFETY: `desc` describes a staging texture; the out-pointer is valid.
            unsafe {
                self.device
                    .CreateTexture2D(&desc, None, Some(&raw mut staging))
            }
            .map_err(os("CreateTexture2D"))?;
            let staging = staging.ok_or_else(|| CaptureError::Os("no staging texture".into()))?;
            *cache = Some((staging, (w, h)));
        }
        let Some((staging, _)) = cache.as_ref() else {
            return Err(CaptureError::Os("no staging texture".into()));
        };
        let source = D3D11_BOX {
            left: x,
            top: y,
            front: 0,
            right: x + w,
            bottom: y + h,
            back: 1,
        };
        // SAFETY: both textures belong to this device; the box lies inside the source (it was
        // clamped to the content size) and the destination has exactly its size.
        unsafe {
            context.CopySubresourceRegion(staging, 0, 0, 0, 0, texture, 0, Some(&raw const source))
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture has CPU read access; `mapped` is a valid out-pointer.
        unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }
            .map_err(os("Map"))?;
        let stride = mapped.RowPitch as usize;
        // SAFETY: a mapped texture exposes `RowPitch * Height` readable bytes until `Unmap`.
        let data =
            unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * h as usize) }
                .to_vec();
        // SAFETY: matches the successful Map above.
        unsafe { context.Unmap(staging, 0) };
        CpuFrame::from_raw(w, h, stride, data)
    }
}

pub struct VideoStream {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    item: GraphicsCaptureItem,
    arrived: i64,
    closed: i64,
    rx: Receiver<Result<CapturedFrame, CaptureError>>,
    size: (u32, u32),
}

impl VideoStream {
    /// `hdr`: ask for scRGB half floats (an HDR monitor); the frames then have `hdr` set.
    pub fn start(target: &StreamTarget, cursor: bool, hdr: bool) -> Result<Self, CaptureError> {
        if !GraphicsCaptureSession::IsSupported().map_err(os("IsSupported"))? {
            return Err(CaptureError::Os(
                "Windows Graphics Capture is not available".into(),
            ));
        }
        let interop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(os("interop"))?;
        let (item, region): (GraphicsCaptureItem, Option<PhysicalRect>) = match target {
            // SAFETY: the handles come from the OS enumeration; a stale one makes the call fail.
            StreamTarget::Monitor(m) => (
                unsafe { interop.CreateForMonitor(HMONITOR(m.id.0 as usize as *mut _)) }
                    .map_err(os("CreateForMonitor"))?,
                None,
            ),
            StreamTarget::Region { monitor, rect } => (
                // SAFETY: as above.
                unsafe { interop.CreateForMonitor(HMONITOR(monitor.id.0 as usize as *mut _)) }
                    .map_err(os("CreateForMonitor"))?,
                Some(*rect),
            ),
            StreamTarget::Window(w) => (
                // SAFETY: as above.
                unsafe { interop.CreateForWindow(HWND(w.0 as usize as *mut _)) }
                    .map_err(os("CreateForWindow"))?,
                None,
            ),
        };
        let size = item.Size().map_err(os("item size"))?;

        let mut device = None;
        let mut context = None;
        // SAFETY: out-pointers are valid; default adapter, no feature-level list.
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

        let shared = Arc::new(Shared {
            device,
            winrt_device: winrt_device.clone(),
            context: Mutex::new(context),
            staging: Mutex::new(None),
            region,
            hdr,
            last_size: Mutex::new(size),
        });
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            pixel_format(hdr),
            2,
            size,
        )
        .map_err(os("CreateFreeThreaded"))?;
        let (tx, rx) = sync_channel(BACKLOG);
        let arrived = pool
            .FrameArrived(&frame_handler(shared, tx.clone()))
            .map_err(os("FrameArrived"))?;
        let closed = item
            .Closed(&TypedEventHandler::new(move |_, _| {
                let _ = tx.try_send(Err(CaptureError::Os(
                    "the captured window was closed".into(),
                )));
                Ok(())
            }))
            .map_err(os("Closed"))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(os("CreateCaptureSession"))?;
        let _ = session.SetIsCursorCaptureEnabled(cursor);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(os("StartCapture"))?;
        let (w, h) = match region {
            Some(r) => (r.width, r.height),
            None => (size.Width.max(1) as u32, size.Height.max(1) as u32),
        };
        Ok(Self {
            pool,
            session,
            item,
            arrived,
            closed,
            rx,
            size: (w, h),
        })
    }

    /// Size of the captured area when the stream started (a window may change it later: read
    /// the size of the frames).
    pub fn source_size(&self) -> (u32, u32) {
        self.size
    }

    /// The next frame; `Ok(None)` when none arrived within `timeout` (nothing changed on screen).
    pub fn recv(&self, timeout: Duration) -> Result<Option<CapturedFrame>, CaptureError> {
        match self.rx.recv_timeout(timeout) {
            Ok(frame) => frame.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Os("capture stopped".into())),
        }
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.arrived);
        let _ = self.item.RemoveClosed(self.closed);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn frame_handler(
    shared: Arc<Shared>,
    tx: SyncSender<Result<CapturedFrame, CaptureError>>,
) -> TypedEventHandler<Direct3D11CaptureFramePool, IInspectable> {
    TypedEventHandler::new(
        move |pool: windows::core::Ref<'_, Direct3D11CaptureFramePool>, _| {
            let Some(pool) = pool.as_ref() else {
                return Ok(());
            };
            // Only the newest frame matters: drain the pool.
            let mut newest = None;
            while let Ok(frame) = pool.TryGetNextFrame() {
                newest = Some(frame);
            }
            let Some(frame) = newest else { return Ok(()) };
            let content = frame.ContentSize()?;
            // A window that was resized needs a pool of the new size.
            if let Ok(mut last) = shared.last_size.lock()
                && (last.Width, last.Height) != (content.Width, content.Height)
            {
                *last = content;
                let _ = pool.Recreate(&shared.winrt_device, pixel_format(shared.hdr), 2, content);
            }
            let time_ns = frame.SystemRelativeTime()?.Duration.saturating_mul(100);
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            // SAFETY: the surface is backed by a D3D11 texture on our device.
            let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
            let result = shared.read(&texture, content).map(|frame| CapturedFrame {
                hdr: shared.hdr,
                time_ns,
                frame,
            });
            // A full queue drops the frame: the consumer is behind.
            let _ = tx.try_send(result);
            Ok(())
        },
    )
}
