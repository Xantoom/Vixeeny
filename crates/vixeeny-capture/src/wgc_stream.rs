// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows Graphics Capture, streaming mode: a capture session that stays open and delivers every frame with the OS
//! monotonic timestamp (`SystemRelativeTime`, the master clock of the recording pipeline) as BGRA in RAM. WGC only
//! produces a frame when the content changes: the consumer repeats the last one to keep a constant frame rate.
//!
//! This is the CPU path (GPU → staging texture → RAM). The zero-copy GPU paths build on the same session: the hardware
//! encoders take each frame as a texture ([`GpuCapture`]), and a viewer on the graphics card takes the newest one when
//! it can ([`VideoStream::start_held`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vixeeny_common::buffers::BufferPool;
use vixeeny_platform::{MonitorInfo, PhysicalRect, WindowId};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface, factory};

use crate::wgc::os;
use crate::{CaptureError, CpuFrame};

/// Frames the consumer may be behind by before new ones are dropped.
const BACKLOG: usize = 3;

fn pixel_format(hdr: bool) -> DirectXPixelFormat {
    if hdr { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized }
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
    /// Nanoseconds on the OS monotonic clock (the same clock as `vixeeny_platform::monotonic_ns`).
    pub time_ns: i64,
    pub frame: CpuFrame,
}

/// Receives each frame as a GPU texture: `(texture, content size, time in ns)`. The texture is only valid during the
/// call (it goes back to the capture pool afterwards): convert or copy it there. Returns whether the frame was taken.
pub type TextureSink = Box<dyn FnMut(&ID3D11Texture2D, (u32, u32), i64) -> bool + Send>;

/// Which frames a recording keeps, asked before a frame is read or converted: a frame the recording would drop costs
/// nothing. Called on a capture thread.
pub trait Gate: Send + Sync {
    /// Whether a frame captured at `time_ns` would be kept.
    fn wanted(&self, time_ns: i64) -> bool;
    /// The frame captured at `time_ns` was handed over.
    fn taken(&self, time_ns: i64);
}

/// The frame rate a recording keeps, and its [`Gate`].
pub struct Pacing {
    /// Frames per second, as `(numerator, denominator)`.
    pub fps: (u32, u32),
    pub gate: Arc<dyn Gate>,
}

/// Told (on a capture thread) that a frame is waiting in [`VideoStream::recv`].
pub type Notify = Box<dyn Fn() + Send + Sync>;

/// The newest frame of a stream, kept on the graphics card (see [`VideoStream::start_held`]). It goes back to the
/// capture pool when dropped: the pool has two, so hold at most one.
pub struct HeldFrame {
    /// The whole picture of the monitor or window, BGRA, on the stream's device.
    pub texture: ID3D11Texture2D,
    /// The part of `texture` that holds the picture (the texture may be larger).
    pub content: (u32, u32),
    /// Nanoseconds on the OS monotonic clock.
    pub time_ns: i64,
    frame: Direct3D11CaptureFrame,
}

impl std::fmt::Debug for HeldFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldFrame")
            .field("content", &self.content)
            .field("time_ns", &self.time_ns)
            .finish_non_exhaustive()
    }
}

impl Drop for HeldFrame {
    fn drop(&mut self) {
        let _ = self.frame.Close();
    }
}

// SAFETY: a D3D11 texture is free-threaded and the frame is a WinRT object (agile).
unsafe impl Send for HeldFrame {}

type Held = Arc<Mutex<Option<HeldFrame>>>;

/// An existing D3D11 device to capture on, and where the textures go (GPU recording).
pub struct GpuCapture {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub sink: TextureSink,
}

/// State shared with the frame callback. The immediate context is not thread safe: one lock.
struct Shared {
    device: ID3D11Device,
    winrt_device: IDirect3DDevice,
    context: Mutex<ID3D11DeviceContext>,
    staging: Mutex<Option<(ID3D11Texture2D, (u32, u32))>>,
    region: Option<PhysicalRect>,
    hdr: bool,
    sink: Option<Mutex<TextureSink>>,
    gate: Option<Arc<dyn Gate>>,
    paused: AtomicBool,
    /// Called once a CPU frame is queued, or a frame is held.
    notify: Option<Notify>,
    /// Where the newest frame waits (see [`VideoStream::start_held`]).
    held: Option<Held>,
    last_size: Mutex<SizeInt32>,
    buffers: BufferPool,
}

// SAFETY: a D3D11 device is free-threaded; the immediate context and the staging texture are only used under their
// mutexes; the other fields are WinRT objects (agile) or plain data.
unsafe impl Send for Shared {}
// SAFETY: see above.
unsafe impl Sync for Shared {}

impl Shared {
    /// Copies `texture` (its `content` part, cropped to the region) into a CPU frame.
    fn read(&self, texture: &ID3D11Texture2D, (cw, ch): (u32, u32)) -> Result<CpuFrame, CaptureError> {
        let (x, y, w, h) = match self.region {
            Some(r) => {
                let x = r.x.max(0) as u32;
                let y = r.y.max(0) as u32;
                (x, y, r.width.min(cw.saturating_sub(x)), r.height.min(ch.saturating_sub(y)))
            }
            None => (0, 0, cw, ch),
        };
        if w == 0 || h == 0 {
            return Err(CaptureError::InvalidRegion);
        }
        let context = self.context.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cache = self.staging.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
            unsafe { self.device.CreateTexture2D(&desc, None, Some(&raw mut staging)) }
                .map_err(os("CreateTexture2D"))?;
            let staging = staging.ok_or_else(|| CaptureError::Os("no staging texture".into()))?;
            *cache = Some((staging, (w, h)));
        }
        let Some((staging, _)) = cache.as_ref() else {
            return Err(CaptureError::Os("no staging texture".into()));
        };
        let source = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
        // SAFETY: both textures belong to this device; the box lies inside the source (it was clamped to the content
        // size) and the destination has exactly its size.
        unsafe { context.CopySubresourceRegion(staging, 0, 0, 0, 0, texture, 0, Some(&raw const source)) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture has CPU read access; `mapped` is a valid out-pointer.
        unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }.map_err(os("Map"))?;
        let stride = mapped.RowPitch as usize;
        // SAFETY: a mapped texture exposes `RowPitch * Height` readable bytes until `Unmap`.
        let pixels = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * h as usize) };
        let mut data = self.buffers.take(pixels.len());
        data.extend_from_slice(pixels);
        // SAFETY: matches the successful Map above.
        unsafe { context.Unmap(staging, 0) };
        CpuFrame::from_raw(w, h, stride, data)
    }
}

/// Where the frames go.
enum Source {
    /// Read into RAM, queued for [`VideoStream::recv`].
    Cpu,
    Sink(GpuCapture),
    Held(ID3D11Device, Held),
}

/// A capture session on one item (a monitor or a window), handing its frames to the stream's [`Shared`].
struct Session {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    item: GraphicsCaptureItem,
    arrived: i64,
    closed: i64,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.arrived);
        let _ = self.item.RemoveClosed(self.closed);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

// SAFETY: WinRT objects (agile) and plain data.
unsafe impl Send for Session {}
// SAFETY: see above.
unsafe impl Sync for Session {}

/// How a session is opened: kept to open another on a new target (see [`VideoStream::retarget`]).
struct Settings {
    cursor: bool,
    /// The shortest time between two frames, in 100 ns ticks.
    min_interval: Option<i64>,
}

pub struct VideoStream {
    session: Mutex<Session>,
    shared: Arc<Shared>,
    settings: Settings,
    tx: SyncSender<Result<CapturedFrame, CaptureError>>,
    rx: Receiver<Result<CapturedFrame, CaptureError>>,
    buffers: BufferPool,
    held: Option<Held>,
}

/// The item `target` is captured through, and the part of it kept.
fn item_of(target: &StreamTarget) -> Result<(GraphicsCaptureItem, Option<PhysicalRect>), CaptureError> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(os("interop"))?;
    Ok(match target {
        // SAFETY: the handles come from the OS enumeration; a stale one makes the call fail.
        StreamTarget::Monitor(m) => (
            unsafe { interop.CreateForMonitor(HMONITOR(m.id.0 as usize as *mut _)) }.map_err(os("CreateForMonitor"))?,
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
            unsafe { interop.CreateForWindow(HWND(w.0 as usize as *mut _)) }.map_err(os("CreateForWindow"))?,
            None,
        ),
    })
}

/// Opens a session on `item`, its frames handed to `shared`.
fn open_session(
    item: GraphicsCaptureItem,
    shared: &Arc<Shared>,
    tx: &SyncSender<Result<CapturedFrame, CaptureError>>,
    settings: &Settings,
) -> Result<Session, CaptureError> {
    let size = item.Size().map_err(os("item size"))?;
    if let Ok(mut last) = shared.last_size.lock() {
        *last = size;
    }
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&shared.winrt_device, pixel_format(shared.hdr), 2, size)
        .map_err(os("CreateFreeThreaded"))?;
    let arrived = pool.FrameArrived(&frame_handler(Arc::clone(shared), tx.clone())).map_err(os("FrameArrived"))?;
    let closing = tx.clone();
    let closed = item
        .Closed(&TypedEventHandler::new(move |_, _| {
            let _ = closing.try_send(Err(CaptureError::Os("the captured window was closed".into())));
            Ok(())
        }))
        .map_err(os("Closed"))?;
    let session = pool.CreateCaptureSession(&item).map_err(os("CreateCaptureSession"))?;
    let _ = session.SetIsCursorCaptureEnabled(settings.cursor);
    let _ = session.SetIsBorderRequired(false);
    if let Some(ticks) = settings.min_interval {
        let _ = session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: ticks });
    }
    session.StartCapture().map_err(os("StartCapture"))?;
    Ok(Session { pool, session, item, arrived, closed })
}

impl VideoStream {
    /// `hdr`: ask for scRGB half floats (an HDR monitor); the frames then have `hdr` set.
    pub fn start(target: &StreamTarget, cursor: bool, hdr: bool) -> Result<Self, CaptureError> {
        Self::start_with(target, cursor, hdr, None, None)
    }

    /// Like [`start`](Self::start); with `gpu`, frames go to its sink as textures and `recv` only reports errors (the
    /// stream ending). With `pacing`, only the frames its gate wants are read or converted.
    pub fn start_with(
        target: &StreamTarget,
        cursor: bool,
        hdr: bool,
        gpu: Option<GpuCapture>,
        pacing: Option<Pacing>,
    ) -> Result<Self, CaptureError> {
        Self::open(target, cursor, hdr, gpu, None, pacing)
    }

    /// Captures on `device` and keeps only the newest frame, on the graphics card, until [`take_held`](Self::take_held)
    /// takes it; `notify` is called each time one waits. No more than one frame per `min_interval` is made (Windows 11
    /// 24H2 and later; older versions make one per change). For a viewer: it shows the newest picture when it can, and
    /// a frame left waiting is replaced, never queued.
    pub fn start_held(
        target: &StreamTarget,
        device: ID3D11Device,
        cursor: bool,
        min_interval: Option<Duration>,
        notify: Notify,
    ) -> Result<Self, CaptureError> {
        let held = Held::default();
        let ticks = min_interval.map(|interval| i64::try_from(interval.as_nanos() / 100).unwrap_or(i64::MAX));
        Self::open_with(target, cursor, false, Source::Held(device, held.clone()), Some(notify), None, ticks)
    }

    /// The newest frame, if one came since the last taken (see [`start_held`](Self::start_held)).
    pub fn take_held(&self) -> Option<HeldFrame> {
        self.held.as_ref()?.lock().ok()?.take()
    }

    /// Like [`start`](Self::start), calling `notify` each time a frame waits in `recv`: the consumer reads them when
    /// told rather than waiting on `recv`.
    pub fn start_notify(target: &StreamTarget, cursor: bool, hdr: bool, notify: Notify) -> Result<Self, CaptureError> {
        Self::open(target, cursor, hdr, None, Some(notify), None)
    }

    fn open(
        target: &StreamTarget,
        cursor: bool,
        hdr: bool,
        gpu: Option<GpuCapture>,
        notify: Option<Notify>,
        pacing: Option<Pacing>,
    ) -> Result<Self, CaptureError> {
        let source = gpu.map_or(Source::Cpu, Source::Sink);
        // Windows 11 24H2 and later: no more than two frames per period of the recording (a screen faster than that
        // skips its other refreshes), so every period still gets a frame. Older versions refuse it and deliver every
        // refresh: the gate sorts them.
        let ticks = pacing.as_ref().map(|p| i64::from(p.fps.1) * 10_000_000 / (2 * i64::from(p.fps.0.max(1))));
        Self::open_with(target, cursor, hdr, source, notify, pacing, ticks)
    }

    fn open_with(
        target: &StreamTarget,
        cursor: bool,
        hdr: bool,
        source: Source,
        notify: Option<Notify>,
        pacing: Option<Pacing>,
        min_interval: Option<i64>,
    ) -> Result<Self, CaptureError> {
        if !GraphicsCaptureSession::IsSupported().map_err(os("IsSupported"))? {
            return Err(CaptureError::Os("Windows Graphics Capture is not available".into()));
        }
        let (item, region) = item_of(target)?;
        let size = item.Size().map_err(os("item size"))?;

        let mut held = None;
        let (device, context, sink) = match source {
            Source::Sink(g) => (g.device, g.context, Some(Mutex::new(g.sink))),
            Source::Held(device, slot) => {
                held = Some(slot);
                // SAFETY: plain call. The context is never used from here: frames are only held.
                let context = unsafe { device.GetImmediateContext() }.map_err(os("GetImmediateContext"))?;
                (device, context, None)
            }
            Source::Cpu => {
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
                (
                    device.ok_or_else(|| CaptureError::Os("no D3D11 device".into()))?,
                    context.ok_or_else(|| CaptureError::Os("no D3D11 context".into()))?,
                    None,
                )
            }
        };
        let dxgi: IDXGIDevice = device.cast().map_err(os("IDXGIDevice"))?;
        // SAFETY: `dxgi` is a live DXGI device.
        let winrt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .and_then(|inspectable| inspectable.cast())
            .map_err(os("CreateDirect3D11DeviceFromDXGIDevice"))?;

        let buffers = BufferPool::default();
        let shared = Arc::new(Shared {
            device,
            winrt_device,
            context: Mutex::new(context),
            staging: Mutex::new(None),
            region,
            hdr,
            sink,
            gate: pacing.as_ref().map(|p| Arc::clone(&p.gate)),
            paused: AtomicBool::new(false),
            notify,
            held: held.clone(),
            last_size: Mutex::new(size),
            buffers: buffers.clone(),
        });
        let (tx, rx) = sync_channel(BACKLOG);
        let settings = Settings { cursor, min_interval };
        let session = open_session(item, &shared, &tx, &settings)?;
        Ok(Self { session: Mutex::new(session), shared, settings, tx, rx, buffers, held })
    }

    /// Captures `target` from now on, in place of what it captured: the frames go on to the same place (the recording
    /// goes on). A monitor or a window only: a part of a monitor stays the one it was opened with.
    pub fn retarget(&self, target: &StreamTarget) -> Result<(), CaptureError> {
        if matches!(target, StreamTarget::Region { .. }) || self.shared.region.is_some() {
            return Err(CaptureError::InvalidRegion);
        }
        let (item, _) = item_of(target)?;
        let session = open_session(item, &self.shared, &self.tx, &self.settings)?;
        // The session before stops once the new one runs: no gap in between.
        let old = self.session.lock().map(|mut current| std::mem::replace(&mut *current, session));
        drop(old);
        Ok(())
    }

    /// Where the pixels of the CPU frames come from: hand them back once used.
    pub fn buffers(&self) -> BufferPool {
        self.buffers.clone()
    }

    /// Discards incoming frames before readback or GPU conversion, including variable-rate recordings.
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
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

fn frame_handler(
    shared: Arc<Shared>,
    tx: SyncSender<Result<CapturedFrame, CaptureError>>,
) -> TypedEventHandler<Direct3D11CaptureFramePool, IInspectable> {
    TypedEventHandler::new(move |pool: windows::core::Ref<'_, Direct3D11CaptureFramePool>, _| {
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
        // A window that was resized needs a pool of the new size, made once this frame (of the old size) is handed
        // over.
        let resized = shared.last_size.lock().is_ok_and(|mut last| {
            let changed = (last.Width, last.Height) != (content.Width, content.Height);
            *last = content;
            changed
        });
        let delivered = deliver(&shared, &tx, frame, content);
        if resized {
            let _ = pool.Recreate(&shared.winrt_device, pixel_format(shared.hdr), 2, content);
        }
        delivered
    })
}

/// Hands `frame` over: held, to the sink, or read into RAM and queued.
fn deliver(
    shared: &Shared,
    tx: &SyncSender<Result<CapturedFrame, CaptureError>>,
    frame: Direct3D11CaptureFrame,
    content: SizeInt32,
) -> windows::core::Result<()> {
    let time_ns = frame.SystemRelativeTime()?.Duration.saturating_mul(100);
    if shared.paused.load(Ordering::Acquire) {
        return Ok(());
    }
    // A frame the recording would drop is neither read nor converted.
    if let Some(gate) = &shared.gate
        && !gate.wanted(time_ns)
    {
        return Ok(());
    }
    let surface = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    // SAFETY: the surface is backed by a D3D11 texture on our device.
    let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
    let content = within(&texture, content);
    if let Some(held) = &shared.held {
        let frame = HeldFrame { texture, content, time_ns, frame };
        // The frame waiting before goes back to the pool.
        let old = held.lock().ok().and_then(|mut slot| slot.replace(frame));
        drop(old);
        if let Some(notify) = &shared.notify {
            notify();
        }
        return Ok(());
    }
    if let Some(sink) = &shared.sink {
        if let Ok(mut sink) = sink.lock()
            && sink(&texture, content, time_ns)
            && let Some(gate) = &shared.gate
        {
            gate.taken(time_ns);
        }
        return Ok(());
    }
    let result = shared.read(&texture, content).map(|frame| CapturedFrame { hdr: shared.hdr, time_ns, frame });
    // A full queue drops the frame: the consumer is behind.
    let read = result.is_ok();
    if queue_cpu(tx, result, &shared.buffers) {
        if read && let Some(gate) = &shared.gate {
            gate.taken(time_ns);
        }
        if let Some(notify) = &shared.notify {
            notify();
        }
    }
    Ok(())
}

/// Hands read-back pixels over, or recycles their allocation when the consumer cannot take them.
fn queue_cpu(
    tx: &SyncSender<Result<CapturedFrame, CaptureError>>,
    result: Result<CapturedFrame, CaptureError>,
    buffers: &BufferPool,
) -> bool {
    match tx.try_send(result) {
        Ok(()) => true,
        Err(TrySendError::Full(result) | TrySendError::Disconnected(result)) => {
            if let Ok(captured) = result {
                buffers.give(captured.frame.data);
            }
            false
        }
    }
}

/// The part of `texture` that holds the picture: `content`, within the texture. A window that grew reports its new size
/// while its frames still come in the textures of the old one, until the pool is made again.
fn within(texture: &ID3D11Texture2D, content: SizeInt32) -> (u32, u32) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { texture.GetDesc(&mut desc) };
    let fit = |size: i32, limit: u32| (size.max(1) as u32).min(limit.max(1));
    (fit(content.Width, desc.Width), fit(content.Height, desc.Height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_streams_drop_new_session_frames_and_resume_without_a_rate_gate() {
        vixeeny_platform::ensure_dpi_aware();
        let Some(monitor) = vixeeny_platform::monitors().ok().and_then(|m| m.into_iter().next()) else {
            vixeeny_common::skip_test("no monitor");
            return;
        };
        let target = StreamTarget::Monitor(monitor);
        let Ok(stream) = VideoStream::start(&target, false, false) else {
            vixeeny_common::skip_test("no capture stream");
            return;
        };
        if stream.recv(Duration::from_secs(2)).ok().flatten().is_none() {
            vixeeny_common::skip_test("no initial frame");
            return;
        }
        stream.set_paused(true);
        // Allow an already in-flight frame from before the pause to finish, then drain it.
        std::thread::sleep(Duration::from_millis(50));
        while stream.recv(Duration::ZERO).expect("stream alive").is_some() {}
        *stream.shared.staging.lock().expect("staging") = None;
        stream.retarget(&target).expect("new paused session");
        assert!(stream.recv(Duration::from_millis(200)).expect("stream alive").is_none());
        assert!(stream.shared.staging.lock().expect("staging").is_none(), "no paused readback texture");
        stream.set_paused(false);
        stream.retarget(&target).expect("new resumed session");
        assert!(stream.recv(Duration::from_secs(2)).expect("stream alive").is_some());
    }

    #[test]
    fn refused_cpu_frames_return_their_pixels_to_the_pool() {
        for disconnected in [false, true] {
            let buffers = BufferPool::default();
            let (tx, rx) = sync_channel(1);
            tx.try_send(Err(CaptureError::InvalidRegion)).unwrap();
            if disconnected {
                drop(rx);
            }
            let frame = CpuFrame::new(64, 64);
            let address = frame.data.as_ptr();
            let captured = CapturedFrame { hdr: false, time_ns: 0, frame };
            assert!(!queue_cpu(&tx, Ok(captured), &buffers));
            let recycled = buffers.take(64 * 64 * 4);
            assert_eq!(recycled.as_ptr(), address);
        }
    }

    /// A window that grew reports more than the texture its frame still comes in: only the texture's pixels are used.
    #[test]
    fn the_content_of_a_frame_stays_within_its_texture() {
        let mut device = None;
        // SAFETY: the out-pointer is valid; default adapter, no feature-level list.
        let made = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                None,
            )
        };
        let (Ok(()), Some(device)) = (made, device) else {
            vixeeny_common::skip_test("no graphics card");
            return;
        };
        let format = windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
        let texture = vixeeny_platform::d3d::texture(&device, (640, 360), format, 0, 0, None).expect("texture");
        let size = |w, h| SizeInt32 { Width: w, Height: h };
        assert_eq!(within(&texture, size(2560, 1440)), (640, 360), "grown");
        assert_eq!(within(&texture, size(320, 1440)), (320, 360), "taller only");
        assert_eq!(within(&texture, size(0, -5)), (1, 1), "minimised");
    }

    /// The editor's live view: the newest frame waits on the graphics card, on the device given, the size of the
    /// monitor, and is replaced rather than queued.
    #[test]
    fn a_held_stream_keeps_the_newest_frame_on_the_device_given() {
        vixeeny_platform::ensure_dpi_aware();
        let Some(monitor) = vixeeny_platform::monitors().ok().and_then(|m| m.into_iter().next()) else {
            vixeeny_common::skip_test("no monitor");
            return;
        };
        let mut device = None;
        // SAFETY: the out-pointer is valid; default adapter, no feature-level list.
        let made = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                None,
            )
        };
        let (Ok(()), Some(device)) = (made, device) else {
            vixeeny_common::skip_test("no graphics card");
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let size = (monitor.rect.width, monitor.rect.height);
        let stream = match VideoStream::start_held(
            &StreamTarget::Monitor(monitor),
            device.clone(),
            false,
            Some(Duration::from_micros(16_667)),
            Box::new(move || {
                let _ = tx.send(());
            }),
        ) {
            Ok(s) => s,
            Err(e) => {
                vixeeny_common::skip_test(&e);
                return;
            }
        };
        // A monitor stream gives a first frame without anything changing.
        if rx.recv_timeout(Duration::from_secs(2)).is_err() {
            vixeeny_common::skip_test("no frame");
            return;
        }
        let frame = stream.take_held().expect("the frame told about waits");
        assert_eq!(frame.content, size);
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a valid out-pointer.
        unsafe { frame.texture.GetDesc(&mut desc) };
        assert_eq!(desc.Format, windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM);
        // SAFETY: plain call.
        let owner = unsafe { frame.texture.GetDevice() }.expect("device");
        assert_eq!(owner, device, "the frame is on the device given");
        assert!(stream.take_held().is_none(), "taken once");
    }

    /// A replay that goes from the game's window to its screen: the same stream, its frames still coming, from the new
    /// source. A part of a monitor cannot change.
    #[test]
    fn a_stream_changes_what_it_captures_and_goes_on() {
        vixeeny_platform::ensure_dpi_aware();
        let Some(monitor) = vixeeny_platform::monitors().ok().and_then(|m| m.into_iter().next()) else {
            vixeeny_common::skip_test("no monitor");
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let stream = match VideoStream::start_notify(
            &StreamTarget::Monitor(monitor.clone()),
            false,
            false,
            Box::new(move || {
                let _ = tx.send(());
            }),
        ) {
            Ok(s) => s,
            Err(e) => {
                vixeeny_common::skip_test(&e);
                return;
            }
        };
        let next = || {
            rx.recv_timeout(Duration::from_secs(2)).ok()?;
            stream.recv(Duration::ZERO).ok().flatten()
        };
        let Some(first) = next() else {
            vixeeny_common::skip_test("no frame");
            return;
        };
        assert_eq!((first.frame.width, first.frame.height), (monitor.rect.width, monitor.rect.height));
        // Opened again on the monitor: a new session gives a first frame at once, from the same stream.
        stream.retarget(&StreamTarget::Monitor(monitor.clone())).expect("retarget");
        let second = next().expect("a frame after the change");
        assert!(second.time_ns > first.time_ns);
        let region = StreamTarget::Region { monitor, rect: PhysicalRect::new(0, 0, 10, 10) };
        assert!(stream.retarget(&region).is_err());
    }
}
