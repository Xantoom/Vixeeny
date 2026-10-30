// SPDX-License-Identifier: GPL-3.0-or-later
//! The frozen screen of a zone capture, done on the GPU by the daemon the moment the shortcut is pressed: every monitor
//! is captured at once (Desktop Duplication, or Windows Graphics Capture where it cannot), the frame is copied into a
//! texture shared with the app and shown 1:1 under a black veil by a borderless top-most window (HDR monitors keep
//! their HDR content: half floats in scRGB). Only the displayed copy is dimmed. Nothing goes through the CPU. The app
//! reads the textures, builds its editor windows hidden, and asks the frozen screens to go once its own windows cover them
//! ([`THAW_MESSAGE`]), so the screen never flickers.

use std::cell::RefCell;
use std::sync::mpsc::channel;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use vixeeny_common::ipc::{Frozen, FrozenScreen, THAW_MESSAGE};
use vixeeny_platform::{HdrInfo, MonitorInfo, PhysicalRect};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, HWND, LPARAM, LRESULT, LUID, LocalFree, WPARAM};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11Device1, ID3D11DeviceContext,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED, DwmFlush, DwmSetWindowAttribute};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_PRESENT, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH, DXGI_SHARED_RESOURCE_READ,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_DISCARD, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGIFactory4, IDXGIResource1, IDXGISwapChain1, IDXGISwapChain3,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IDC_ARROW, LoadCursorW, MA_NOACTIVATEANDEAT, PostMessageW,
    RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SetTimer, SetWindowDisplayAffinity, ShowWindow,
    WDA_EXCLUDEFROMCAPTURE, WM_MOUSEACTIVATE, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, Interface, w};

use crate::wgc::{FRAME_TIMEOUT, d3d_device, frame_handler, halves_to_f32, monitor_item, os, winrt_device};

use crate::freeze_veil::Veil;
use crate::{CaptureError, CpuFrame, HdrFrame, card_tonemap};

/// The frozen screens go by themselves after this, whatever happens to the app.
const SAFETY_TIMEOUT_MS: u32 = 10_000;

/// A Direct3D device kept ready by the daemon (creating one costs tens of milliseconds).
pub struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
    factory: IDXGIFactory2,
    adapter: i64,
    veil: Veil,
}

fn luid_value(luid: LUID) -> i64 {
    (i64::from(luid.HighPart) << 32) | i64::from(luid.LowPart)
}

impl Gpu {
    pub fn new() -> Result<Self, CaptureError> {
        if !GraphicsCaptureSession::IsSupported().map_err(os("IsSupported"))? {
            return Err(CaptureError::Os("Windows Graphics Capture is not available".into()));
        }
        let (device, context) = d3d_device(None)?;
        let winrt = winrt_device(&device)?;
        let dxgi: IDXGIDevice = device.cast().map_err(os("IDXGIDevice"))?;
        // SAFETY: plain queries on live DXGI objects.
        let (factory, adapter) = unsafe {
            let adapter = dxgi.GetAdapter().map_err(os("GetAdapter"))?;
            let desc = adapter.GetDesc().map_err(os("adapter description"))?;
            let factory: IDXGIFactory2 = adapter.GetParent().map_err(os("DXGI factory"))?;
            (factory, luid_value(desc.AdapterLuid))
        };
        let veil = Veil::new(&device)?;
        register_class()?;
        Ok(Self { device, context, winrt, factory, adapter, veil })
    }
}

/// One frozen monitor: its window and swap chain, and the texture the app reads.
struct Screen {
    window: HWND,
    swap_chain: Option<IDXGISwapChain1>,
    texture: ID3D11Texture2D,
    id: u64,
    /// The named handle the app opens the texture by; the name goes with it.
    shared: Option<HANDLE>,
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Some(handle) = self.shared.take() {
            // SAFETY: a handle this screen created and closes once.
            let _ = unsafe { CloseHandle(handle) };
        }
        self.swap_chain = None;
        // SAFETY: the window belongs to this thread and is destroyed once.
        let _ = unsafe { DestroyWindow(self.window) };
    }
}

thread_local! {
    static ACTIVE: RefCell<Vec<Screen>> = const { RefCell::new(Vec::new()) };
    /// The device the frozen screens were made on, trimmed once they go.
    static MADE_ON: RefCell<Option<ID3D11Device>> = const { RefCell::new(None) };
}

/// `true` while frozen screens are shown.
pub fn is_active() -> bool {
    ACTIVE.with(|a| !a.borrow().is_empty())
}

/// Removes the frozen screens (no-op when there are none).
pub fn thaw() {
    let screens = ACTIVE.with(|a| std::mem::take(&mut *a.borrow_mut()));
    for s in &screens {
        // SAFETY: windows of this thread; hiding them all first makes them go together.
        let _ = unsafe { ShowWindow(s.window, SW_HIDE) };
    }
    let any = !screens.is_empty();
    drop(screens);
    // The driver keeps the memory of the textures and swap chains it freed (over 100 MB for a 4K screen) for later
    // ones: given back now, at about a millisecond on the next freeze.
    if any && let Some(device) = MADE_ON.with(|m| m.borrow_mut().take()) {
        crate::trim(&device);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        THAW_MESSAGE | WM_TIMER => {
            thaw();
            LRESULT(0)
        }
        // A click before the editor is up must not reach the window below.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATEANDEAT as isize),
        // SAFETY: forwards the arguments we received, unchanged.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

const CLASS: windows::core::PCWSTR = w!("VixeenyFrozenScreen");

fn register_class() -> Result<(), CaptureError> {
    // Classes belong to the process, even when devices/windows are created on different threads (including tests).
    static REGISTERED: OnceLock<windows::core::Result<()>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            // SAFETY: a static class name and a window procedure with the required signature.
            unsafe {
                let instance = GetModuleHandleW(None)?;
                let wc = WNDCLASSEXW {
                    cbSize: size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance.into(),
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    lpszClassName: CLASS,
                    ..Default::default()
                };
                if RegisterClassExW(&wc) == 0 {
                    return Err(windows::core::Error::from_thread());
                }
            }
            Ok(())
        })
        .as_ref()
        .copied()
        .map_err(os("register frozen window class"))
}

fn set_bool_attribute(hwnd: HWND, attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE, on: bool) {
    let value = BOOL::from(on);
    // SAFETY: `value` is a live BOOL, the size these attributes want.
    let _ = unsafe { DwmSetWindowAttribute(hwnd, attribute, (&raw const value).cast(), size_of::<BOOL>() as u32) };
}

/// A capture in flight.
struct Pending {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: i64,
    arrived: std::sync::mpsc::Receiver<()>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn start(gpu: &Gpu, monitor: &MonitorInfo, cursor: bool) -> Result<Pending, CaptureError> {
    let item = monitor_item(monitor)?;
    let format = if monitor.hdr.is_some() {
        DirectXPixelFormat::R16G16B16A16Float
    } else {
        DirectXPixelFormat::B8G8R8A8UIntNormalized
    };
    let size = item.Size().map_err(os("item size"))?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&gpu.winrt, format, 1, size)
        .map_err(os("CreateFreeThreaded"))?;
    let (tx, arrived) = channel();
    let token = pool.FrameArrived(&frame_handler(tx)).map_err(os("FrameArrived"))?;
    let session = pool.CreateCaptureSession(&item).map_err(os("CreateCaptureSession"))?;
    let pending = Pending { pool, session, token, arrived };
    // The cursor and border switches need recent Windows builds; ignore their absence.
    let _ = pending.session.SetIsCursorCaptureEnabled(cursor);
    let _ = pending.session.SetIsBorderRequired(false);
    pending.session.StartCapture().map_err(os("StartCapture"))?;
    Ok(pending)
}

/// Waits for the first frame and copies it into the already prepared shared texture.
fn finish(gpu: &Gpu, pending: &Pending, deadline: Instant, screen: &Screen) -> Result<(), CaptureError> {
    pending
        .arrived
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| CaptureError::Timeout)?;
    let frame = pending.pool.TryGetNextFrame().map_err(os("TryGetNextFrame"))?;
    let surface = frame.Surface().map_err(os("Surface"))?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(os("surface access"))?;
    // SAFETY: the surface is backed by a D3D11 texture on our device.
    let source: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(os("GetInterface"))?;
    let result = copy_frame(gpu, &source, &screen.texture);
    let _ = frame.Close();
    result
}

/// A display mode change must not copy a differently sized/formatted frame into a prepared texture.
fn copy_frame(gpu: &Gpu, source: &ID3D11Texture2D, destination: &ID3D11Texture2D) -> Result<(), CaptureError> {
    let (mut from, mut to) = (D3D11_TEXTURE2D_DESC::default(), D3D11_TEXTURE2D_DESC::default());
    // SAFETY: live textures and valid out-pointers.
    unsafe {
        source.GetDesc(&mut from);
        destination.GetDesc(&mut to);
    }
    if (from.Width, from.Height, from.Format) != (to.Width, to.Height, to.Format) {
        return Err(CaptureError::Os("the display mode changed during capture".into()));
    }
    // SAFETY: same device, size and format; neither texture is mapped.
    unsafe { gpu.context.CopyResource(destination, source) };
    Ok(())
}

/// Sessions are created for every monitor before any frame is waited for.
enum Started {
    Duplicating(crate::dda::Pending),
    Capturing(Pending),
}

/// A monitor's picture on its way: copied already, or captured once its frame comes.
enum Source {
    Copied,
    Captured(Pending),
}

/// A hidden (cloaked) top-most window over `monitor`, ready for a picture when `show` (otherwise it only receives
/// [`THAW_MESSAGE`] and the safety timer).
///
/// The window must stay an ordinary window for the compositor. One that covers a monitor exactly with a flip-model swap
/// chain is taken for a full-screen game: the screen switches to "independent flip" (a flash, and variable refresh that
/// follows our single frame). So an SDR screen uses a "blt" swap chain, which the compositor always composes, and the
/// window is one pixel taller than the monitor when no monitor lies below. HDR content needs the flip model.
fn prepare_screen(
    gpu: &Gpu,
    monitor: &MonitorInfo,
    monitors: &[MonitorInfo],
    show: bool,
) -> Result<Screen, CaptureError> {
    let r = monitor.rect;
    let hdr = monitor.hdr.is_some();
    let texture = vixeeny_platform::d3d::texture(
        &gpu.device,
        (r.width, r.height),
        if hdr { DXGI_FORMAT_R16G16B16A16_FLOAT } else { DXGI_FORMAT_B8G8R8A8_UNORM },
        (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        SHARED_FLAGS,
        None,
    )?;
    let bottom = r.y + r.height as i32;
    let below_is_free = !monitors.iter().any(|m| {
        m.rect.y <= bottom
            && bottom < m.rect.y + m.rect.height as i32
            && m.rect.x < r.x + r.width as i32
            && r.x < m.rect.x + m.rect.width as i32
    });
    let extra = u32::from(below_is_free);
    let ex_style = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let ex_style = if hdr { ex_style | WS_EX_NOREDIRECTIONBITMAP } else { ex_style };
    // SAFETY: a registered class and plain arguments.
    let window = unsafe {
        CreateWindowExW(
            ex_style,
            CLASS,
            w!("Vixeeny"),
            WS_POPUP,
            r.x,
            r.y,
            r.width as i32,
            (r.height + extra) as i32,
            None,
            None,
            Some(GetModuleHandleW(None).map_err(os("GetModuleHandleW"))?.into()),
            None,
        )
    }
    .map_err(os("CreateWindowExW"))?;
    let mut screen = Screen { window, swap_chain: None, texture, id: next_shared_id(), shared: None };
    set_bool_attribute(window, DWMWA_TRANSITIONS_FORCEDISABLED, true);
    set_bool_attribute(window, DWMWA_CLOAK, true);
    // SAFETY: our own window. Kept out of captures, so a capture never sees a frozen screen.
    let _ = unsafe { SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE) };
    screen.shared = Some(share(&screen.texture, screen.id)?);
    if show {
        screen.swap_chain = Some(swap_chain(gpu, window, r.width, r.height + extra, hdr)?);
        // SAFETY: shown while cloaked, before capture. Hidden windows' frames would be dropped by Present.
        let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
    }
    Ok(screen)
}

fn show_screen(gpu: &Gpu, screen: &mut Screen, monitor: &MonitorInfo) -> Result<FrozenScreen, CaptureError> {
    let r = monitor.rect;
    if let Some(chain) = &screen.swap_chain {
        // SAFETY: a live swap chain, with the captured texture's format and size (plus possibly one row).
        unsafe {
            let back: ID3D11Texture2D = chain.GetBuffer(0).map_err(os("GetBuffer"))?;
            gpu.context.CopySubresourceRegion(&back, 0, 0, 0, 0, &screen.texture, 0, None);
            gpu.veil.draw(&gpu.device, &gpu.context, &back, r.width, r.height)?;
            chain.Present(0, DXGI_PRESENT(0)).ok().map_err(os("Present"))?;
        }
    }
    let shared = FrozenScreen {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
        texture: screen.id,
        hdr: monitor.hdr.is_some(),
        window: screen.window.0 as usize as u64,
    };
    Ok(shared)
}

/// Creates the expensive window resources before sampling the desktop, shortening the age of the frame at display.
fn swap_chain(gpu: &Gpu, window: HWND, width: u32, height: u32, hdr: bool) -> Result<IDXGISwapChain1, CaptureError> {
    let chain_desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: if hdr { DXGI_FORMAT_R16G16B16A16_FLOAT } else { DXGI_FORMAT_B8G8R8A8_UNORM },
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: if hdr { 2 } else { 1 },
        Scaling: if hdr { DXGI_SCALING_NONE } else { DXGI_SCALING_STRETCH },
        SwapEffect: if hdr { DXGI_SWAP_EFFECT_FLIP_DISCARD } else { DXGI_SWAP_EFFECT_DISCARD },
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        ..Default::default()
    };
    // SAFETY: a live device and window; the description outlives the call.
    let chain = unsafe { gpu.factory.CreateSwapChainForHwnd(&gpu.device, window, &chain_desc, None, None) }
        .map_err(os("CreateSwapChainForHwnd"))?;
    if hdr {
        // Linear BT.709 floats, 1.0 = 80 nits: what the capture gave, shown as is.
        if let Ok(chain3) = chain.cast::<IDXGISwapChain3>() {
            // SAFETY: a plain call on a live swap chain.
            let _ = unsafe { chain3.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709) };
        }
    }
    Ok(chain)
}

/// Freezes every monitor. Must run on a thread with a message loop (the windows live there). The screens go on
/// [`thaw`], on [`THAW_MESSAGE`] or after ten seconds.
pub fn freeze(gpu: &Gpu, monitors: &[MonitorInfo], cursor: bool) -> Result<Frozen, CaptureError> {
    take(gpu, monitors, true, cursor)
}

/// Captures every monitor for the app without showing anything (a full-screen capture takes the screen of the moment
/// the shortcut is pressed, however long the app takes to start). Shared and released as frozen screens are.
pub fn hold(gpu: &Gpu, monitors: &[MonitorInfo], cursor: bool) -> Result<Frozen, CaptureError> {
    take(gpu, monitors, false, cursor)
}

fn take(gpu: &Gpu, monitors: &[MonitorInfo], show: bool, cursor: bool) -> Result<Frozen, CaptureError> {
    let began = Instant::now();
    thaw();
    register_class()?;
    let mut screens = monitors.iter().map(|m| prepare_screen(gpu, m, monitors, show)).collect::<Result<Vec<_>, _>>()?;
    let prepared = began.elapsed();
    let capture_started = Instant::now();
    // Duplicated when they can be (without the pointer); every capture starts before any is waited for: they run side
    // by side.
    let started = monitors
        .iter()
        .map(|m| {
            let duplication = (!cursor).then(|| crate::dda::start(&gpu.device, m, m.hdr.is_some())).flatten();
            match duplication {
                Some(duplication) => Ok(Started::Duplicating(duplication)),
                None => start(gpu, m, cursor).map(Started::Capturing),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let duplication_deadline = Instant::now() + crate::dda::DESKTOP_WAIT;
    // Start every WGC fallback before waiting for any WGC frame too. Copy a duplication while it is owned, before
    // releasing it; waiting for another monitor must never invalidate that source texture.
    let pending = started
        .into_iter()
        .zip(monitors.iter().zip(&screens))
        .map(|(source, (monitor, screen))| match source {
            Started::Capturing(p) => Ok(Source::Captured(p)),
            Started::Duplicating(p) => match p.finish(duplication_deadline) {
                Some(frame) => copy_frame(gpu, &frame.texture, &screen.texture).map(|()| Source::Copied),
                None => start(gpu, monitor, cursor).map(Source::Captured),
            },
        })
        .collect::<Result<Vec<_>, _>>()?;
    let deadline = Instant::now() + FRAME_TIMEOUT;
    pending.iter().zip(&screens).try_for_each(|(p, screen)| match p {
        Source::Copied => Ok(()),
        Source::Captured(p) => finish(gpu, p, deadline, screen),
    })?;
    let captured = capture_started.elapsed();
    let duplicated = pending.iter().filter(|p| matches!(p, Source::Copied)).count();
    let present_started = Instant::now();

    let mut shared = Vec::with_capacity(monitors.len());
    for (monitor, screen) in monitors.iter().zip(&mut screens) {
        shared.push(show_screen(gpu, screen, monitor)?);
    }
    // SAFETY: plain call: the copies reach the GPU before the app reads them.
    unsafe { gpu.context.Flush() };
    if show {
        // SAFETY: plain call. Once the compositor has taken the presented frames, the windows appear all at once with
        // their content.
        let _ = unsafe { DwmFlush() };
        for s in &screens {
            set_bool_attribute(s.window, DWMWA_CLOAK, false);
        }
    }
    if let Some(first) = screens.first() {
        // SAFETY: a timer on our own window; its WM_TIMER thaws.
        unsafe { SetTimer(Some(first.window), 1, SAFETY_TIMEOUT_MS, None) };
    }
    ACTIVE.with(|a| *a.borrow_mut() = screens);
    MADE_ON.with(|m| *m.borrow_mut() = Some(gpu.device.clone()));
    tracing::debug!(
        screens = monitors.len(),
        cursor,
        show,
        duplicated,
        wgc = monitors.len() - duplicated,
        ?prepared,
        ?captured,
        present = ?present_started.elapsed(),
        total = ?began.elapsed(),
        "frozen screens submitted to the compositor"
    );
    // Closing WGC sessions can block. The frozen windows must be visible before that cleanup, rather than showing
    // a picture that has aged while the live desktop kept moving.
    drop(pending);
    Ok(Frozen { adapter: gpu.adapter, screens: shared })
}

/// The rows of a frozen HDR screen (RGBA half floats, `stride` bytes apart, as [`read_with`] hands them) as scRGB.
pub fn scrgb(screen: &FrozenScreen, rows: &[u8], stride: usize) -> HdrFrame {
    let (w, h) = (screen.width, screen.height);
    HdrFrame { width: w, height: h, rgba: halves_to_f32(rows, stride, w as usize, h as usize) }
}

/// The app's device on the daemon's adapter.
struct Reader {
    adapter: i64,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

// SAFETY: a Direct3D 11 device is free-threaded; the immediate context is used by one thread at a time (made on the
// warming thread, then used by the thread that reads the screens).
unsafe impl Send for Reader {}

/// The reader, kept for the next captures of the process.
static READER: Mutex<Option<Reader>> = Mutex::new(None);

/// The reader being made in the background (see [`warm`]).
static WARMING: Mutex<Option<JoinHandle<Result<Reader, CaptureError>>>> = Mutex::new(None);

/// Starts making the device that reads the frozen screens of `frozen`, while the app starts.
pub fn warm(frozen: &Frozen) {
    let adapter = frozen.adapter;
    let made = std::thread::Builder::new().name("frozen-warm".into()).spawn(move || make_reader(adapter));
    if let (Ok(handle), Ok(mut warming)) = (made, WARMING.lock()) {
        *warming = Some(handle);
    }
}

fn reader(adapter: i64) -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    let mut kept = READER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: plain call.
    let removed = |r: &Reader| unsafe { r.device.GetDeviceRemovedReason() }.is_err();
    if kept.as_ref().is_none_or(|r| r.adapter != adapter || removed(r)) {
        let warming = WARMING.lock().ok().and_then(|mut w| w.take());
        *kept = Some(match warming.map(JoinHandle::join) {
            Some(Ok(Ok(made))) if made.adapter == adapter => made,
            _ => make_reader(adapter)?,
        });
    }
    let r = kept.as_ref().ok_or_else(|| CaptureError::Os("no reader".into()))?;
    Ok((r.device.clone(), r.context.clone()))
}

/// Gives back the driver's memory of the device that read the frozen screens, once the editor that drew with it is gone
/// (see [`crate::trim`]).
pub fn trim_reader() {
    if let Some(device) = READER.lock().ok().and_then(|r| r.as_ref().map(|r| r.device.clone())) {
        crate::trim(&device);
    }
}

/// The device that reads the frozen screens of `adapter` (made if needed): the editor draws with it too, rather than
/// making one more.
pub fn device(adapter: i64) -> Option<ID3D11Device> {
    reader(adapter).ok().map(|(device, _)| device)
}

fn make_reader(adapter: i64) -> Result<Reader, CaptureError> {
    let luid = LUID { LowPart: adapter as u32, HighPart: (adapter >> 32) as i32 };
    // SAFETY: plain factory calls.
    let dxgi_adapter: IDXGIAdapter = unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory1().map_err(os("CreateDXGIFactory1"))?;
        factory.EnumAdapterByLuid(luid)
    }
    .map_err(os("EnumAdapterByLuid"))?;
    let (device, context) = d3d_device(Some(&dxgi_adapter))?;
    Ok(Reader { adapter, device, context })
}

/// How the frozen textures are created: shared through a named NT handle, which has an access control list (a legacy
/// shared handle is a bare number any process can try).
const SHARED_FLAGS: u32 = (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32;

/// Only this user and the system may open a frozen screen, and not a process of a lower integrity level (a sandboxed
/// one).
const SHARED_SDDL: windows::core::PCWSTR = w!("D:P(A;;GA;;;OW)(A;;GA;;;SY)S:(ML;;NWNRNX;;;ME)");

/// A name no other frozen texture has: this process and a counter.
fn next_shared_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    (u64::from(std::process::id()) << 32) | u64::from(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

fn shared_name(id: u64) -> windows::core::HSTRING {
    windows::core::HSTRING::from(format!("Vixeeny-frozen-{id:016x}"))
}

/// Shares `texture` under the name of `id`; the name lasts as long as the handle.
fn share(texture: &ID3D11Texture2D, id: u64) -> Result<HANDLE, CaptureError> {
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: a constant SDDL string; the descriptor is freed below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(SHARED_SDDL, SDDL_REVISION_1, &raw mut descriptor, None)
    }
    .map_err(os("security descriptor"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let resource: Result<IDXGIResource1, _> = texture.cast().map_err(os("IDXGIResource1"));
    let handle = resource.and_then(|resource| {
        // SAFETY: a texture created with `SHARED_FLAGS`; `attributes` outlives the call.
        unsafe {
            resource.CreateSharedHandle(Some(&raw const attributes), DXGI_SHARED_RESOURCE_READ.0, &shared_name(id))
        }
        .map_err(os("CreateSharedHandle"))
    });
    // SAFETY: allocated by the conversion above, freed once.
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    handle
}

/// Opens the frozen texture of `id` (in the app); one already released fails.
fn open_shared(device: &ID3D11Device, id: u64) -> Result<ID3D11Texture2D, CaptureError> {
    let device: ID3D11Device1 = device.cast().map_err(os("ID3D11Device1"))?;
    // SAFETY: a name, and the access the daemon shared it with.
    unsafe { device.OpenSharedResourceByName(&shared_name(id), DXGI_SHARED_RESOURCE_READ.0) }
        .map_err(os("OpenSharedResourceByName"))
}

/// How the rows [`read_with`] hands are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rows {
    /// BGRA bytes: an SDR screen, or an HDR one tone-mapped on the graphics card.
    Bgra,
    /// RGBA half floats (scRGB): an HDR screen as it is.
    Halves,
}

/// Reads the shared textures (in the app): `each` gets every screen's rows (`stride` bytes apart) while they are
/// mapped, so it can convert them straight to where they go. The GPU work of all screens is queued before the first is
/// waited for.
///
/// An HDR screen for which `sdr` gives its levels is tone-mapped to BGRA on the graphics card; it comes as half floats
/// too with `halves`, or when that tone mapping failed (and always after its BGRA rows).
pub fn read_with(
    frozen: &Frozen,
    sdr: impl Fn(&FrozenScreen) -> Option<HdrInfo>,
    halves: bool,
    mut each: impl FnMut(&FrozenScreen, Rows, &[u8], usize),
) -> Result<(), CaptureError> {
    let result = (|| {
        let (device, context) = reader(frozen.adapter)?;
        let mut staged = Vec::with_capacity(frozen.screens.len() * 2);
        for screen in &frozen.screens {
            let texture = open_shared(&device, screen.texture)?;
            let whole = PhysicalRect::new(0, 0, screen.width, screen.height);
            let card = sdr(screen)
                .filter(|_| screen.hdr)
                .and_then(|info| card_tonemap::stage(&device, &context, &texture, &whole, &info).ok());
            let on_card = card.is_some();
            if let Some(staging) = card {
                staged.push((screen, Rows::Bgra, staging));
            }
            if !on_card || halves && screen.hdr {
                let rows = if screen.hdr { Rows::Halves } else { Rows::Bgra };
                staged.push((screen, rows, stage(&device, &context, &texture)?));
            }
        }
        for (screen, rows, staging) in staged {
            let read = |data: &[u8], stride| each(screen, rows, data, stride);
            vixeeny_platform::d3d::read_staging(&context, &staging, screen.height, read)?;
        }
        Ok(())
    })();
    if result.is_err() {
        // The device may be lost: the next capture starts with a new one.
        if let Ok(mut kept) = READER.lock() {
            *kept = None;
        }
    }
    result
}

/// A staging copy of `texture`, queued on the GPU.
fn stage(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11Texture2D, CaptureError> {
    let staging = vixeeny_platform::d3d::staging_for(device, texture, None)?;
    // SAFETY: same device, size and format.
    unsafe { context.CopyResource(&staging, texture) };
    Ok(staging)
}

/// Asks the daemon to remove the frozen screens (in the app, once its windows cover them).
pub fn release(frozen: &Frozen) {
    if let Some(screen) = frozen.screens.first() {
        let hwnd = HWND(screen.window as usize as *mut _);
        // SAFETY: posting to another process's window; a stale handle just fails.
        let _ = unsafe { PostMessageW(Some(hwnd), THAW_MESSAGE, WPARAM(0), LPARAM(0)) };
    }
}

/// Still captures of the screens the daemon held for the app ([`hold`]): a full-screen capture of the moment of the
/// press. Windows are not held (their capture sees behind what covers them): asking for one fails.
pub struct Held<'a> {
    frozen: &'a Frozen,
}

impl<'a> Held<'a> {
    pub fn new(frozen: &'a Frozen) -> Self {
        Self { frozen }
    }

    /// The held screen of `monitor`, alone; none when the monitors moved since the press.
    fn screen(&self, monitor: &MonitorInfo) -> Result<Frozen, CaptureError> {
        let r = monitor.rect;
        self.frozen
            .screens
            .iter()
            .find(|s| (s.x, s.y, s.width, s.height) == (r.x, r.y, r.width, r.height))
            .map(|s| Frozen { adapter: self.frozen.adapter, screens: vec![*s] })
            .ok_or(CaptureError::UnknownMonitor)
    }

    /// `monitor`'s screen as BGRA (tone-mapped on the graphics card when `sdr` gives the levels of an HDR one) or,
    /// failing that, as scRGB.
    fn read(&self, monitor: &MonitorInfo, sdr: Option<HdrInfo>) -> Result<Result<CpuFrame, HdrFrame>, CaptureError> {
        let one = self.screen(monitor)?;
        let mut out = None;
        read_with(
            &one,
            |_| sdr,
            false,
            |screen, rows, data, stride| {
                out = Some(match rows {
                    Rows::Bgra => CpuFrame::from_raw(screen.width, screen.height, stride, data.to_vec()).map(Ok),
                    Rows::Halves => Ok(Err(scrgb(screen, data, stride))),
                });
            },
        )?;
        out.ok_or(CaptureError::Unsupported)?
    }
}

fn cropped<T>(
    frame: T,
    area: Option<&PhysicalRect>,
    crop: impl FnOnce(&T, &PhysicalRect) -> Result<T, CaptureError>,
) -> Result<T, CaptureError> {
    match area {
        Some(area) => crop(&frame, area),
        None => Ok(frame),
    }
}

impl crate::StillBackend for Held<'_> {
    fn grab_monitor(&mut self, monitor: &MonitorInfo, _cursor: bool) -> Result<CpuFrame, CaptureError> {
        self.read(monitor, None)?.map_err(|_| CaptureError::Unsupported)
    }

    fn grab_monitor_hdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        _cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        match self.read(monitor, None)? {
            Err(hdr) => cropped(hdr, area, HdrFrame::crop),
            Ok(_) => Err(CaptureError::Unsupported),
        }
    }

    fn grab_monitor_sdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        _cursor: bool,
        info: &HdrInfo,
        tonemap: crate::ToneMapFn,
    ) -> Result<CpuFrame, CaptureError> {
        let sdr = match self.read(monitor, Some(*info))? {
            Ok(sdr) => sdr,
            Err(hdr) => crate::tonemapped(&hdr, info, tonemap)?,
        };
        cropped(sdr, area, CpuFrame::crop)
    }

    fn grab_window(&mut self, _window: vixeeny_platform::WindowId, _cursor: bool) -> Result<CpuFrame, CaptureError> {
        Err(CaptureError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Taken by the tests reading through the process's reader: its immediate context is used by one thread at a time,
    /// as in the app.
    static READING: Mutex<()> = Mutex::new(());

    /// The screens held at a press (nothing shown) are read back whole, as the app saves them.
    #[test]
    fn held_screens_are_read_back() {
        use crate::StillBackend;
        let _reading = READING.lock();
        // Physical pixels, as the daemon and the app see the monitors.
        vixeeny_platform::ensure_dpi_aware();
        let (Ok(gpu), Ok(monitors)) = (Gpu::new(), vixeeny_platform::monitors()) else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let started = Instant::now();
        let frozen = hold(&gpu, &monitors, false).unwrap();
        eprintln!("held in {:?}", started.elapsed());
        assert_eq!(frozen.screens.len(), monitors.len());
        let monitor = &monitors[0];
        let mut held = Held::new(&frozen);
        let frame = match monitor.hdr {
            Some(info) => held.grab_monitor_sdr(monitor, None, false, &info, |_, _| Vec::new()),
            None => held.grab_monitor(monitor, false),
        }
        .unwrap();
        assert_eq!((frame.width, frame.height), (monitor.rect.width, monitor.rect.height));
        thaw();
        assert!(!is_active());
    }

    /// A texture shared by one device is read by another, as the app reads the daemon's.
    #[test]
    fn shared_textures_are_read_back() {
        let _reading = READING.lock();
        let Ok(gpu) = Gpu::new() else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let (w, h) = (3u32, 2u32);
        let pixels: Vec<u8> = (0..w * h * 4).map(|i| i as u8).collect();
        let bind = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
        let texture = vixeeny_platform::d3d::texture(
            &gpu.device,
            (w, h),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            bind,
            SHARED_FLAGS,
            Some((&pixels, w * 4)),
        )
        .unwrap();
        // SAFETY: the context of the device that made the texture.
        unsafe { gpu.context.Flush() };
        let id = next_shared_id();
        let handle = share(&texture, id).unwrap();
        let frozen = Frozen {
            adapter: gpu.adapter,
            screens: vec![FrozenScreen { x: 0, y: 0, width: w, height: h, texture: id, hdr: false, window: 0 }],
        };
        let mut read: Vec<u8> = Vec::new();
        read_with(
            &frozen,
            |_| None,
            false,
            |_, kind, rows, stride| {
                assert_eq!(kind, Rows::Bgra);
                read.extend((0..h as usize).flat_map(|y| &rows[y * stride..y * stride + w as usize * 4]))
            },
        )
        .unwrap();
        let at = |x: usize, y: usize| &read[(y * w as usize + x) * 4..][..4];
        assert_eq!(at(2, 1), [20, 21, 22, 23]);
        assert_eq!(at(0, 0), [0, 1, 2, 3]);
        // Once the daemon lets the texture go, its name opens nothing.
        // SAFETY: the handle `share` returned, closed once.
        let _ = unsafe { CloseHandle(handle) };
        drop(texture);
        assert!(read_with(&frozen, |_| None, false, |_, _, _, _| {}).is_err());
    }

    #[test]
    fn frozen_hdr_screens_read_as_scrgb() {
        let one = 0x3c00u16.to_le_bytes(); // 1.0
        let half = 0x3800u16.to_le_bytes(); // 0.5
        // One pixel, then row padding.
        let rows = [one, half, [0, 0], one, [9, 9], [9, 9], [9, 9], [9, 9]].concat();
        let screen = FrozenScreen { x: 0, y: 0, width: 1, height: 1, texture: 0, hdr: true, window: 0 };
        assert_eq!(scrgb(&screen, &rows, 16).rgba, vec![1.0, 0.5, 0.0, 1.0]);
    }
}
