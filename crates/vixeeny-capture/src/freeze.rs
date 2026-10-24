// SPDX-License-Identifier: GPL-3.0-or-later
//! The frozen screen of a zone capture, done on the GPU by the daemon the moment the
//! shortcut is pressed: every monitor is captured at once (Windows Graphics Capture), the frame
//! is copied into a texture shared with the app and shown 1:1 by a borderless top-most window
//! with its own flip-model swap chain (HDR monitors keep their HDR content: half floats in
//! scRGB). Nothing goes through the CPU. The app reads the textures, builds its editor windows
//! hidden, and asks the frozen screens to go once its own windows cover them
//! ([`THAW_MESSAGE`]), so the screen never flickers.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Instant;

use vixeeny_common::ipc::{Frozen, FrozenScreen, THAW_MESSAGE};
use vixeeny_platform::{HdrInfo, MonitorInfo, PhysicalRect};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HLOCAL, HWND, LPARAM, LRESULT, LUID, LocalFree, WPARAM,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, ID3D11Device, ID3D11Device1,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED, DwmFlush, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_PRESENT, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH,
    DXGI_SHARED_RESOURCE_READ, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_DISCARD,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIDevice,
    IDXGIDevice3, IDXGIFactory2, IDXGIFactory4, IDXGIResource1, IDXGISwapChain1, IDXGISwapChain3,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IDC_ARROW, LoadCursorW, MA_NOACTIVATEANDEAT,
    PostMessageW, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SetTimer, SetWindowDisplayAffinity,
    ShowWindow, WDA_EXCLUDEFROMCAPTURE, WM_MOUSEACTIVATE, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, Interface, w};

use crate::wgc::{
    FRAME_TIMEOUT, d3d_device, frame_handler, halves_to_f32, monitor_item, os, winrt_device,
};

use crate::{CaptureError, HdrFrame, card_tonemap};

/// The frozen screens go by themselves after this, whatever happens to the app.
const SAFETY_TIMEOUT_MS: u32 = 10_000;

/// A Direct3D device kept ready by the daemon (creating one costs tens of milliseconds).
pub struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
    factory: IDXGIFactory2,
    adapter: i64,
}

fn luid_value(luid: LUID) -> i64 {
    (i64::from(luid.HighPart) << 32) | i64::from(luid.LowPart)
}

impl Gpu {
    pub fn new() -> Result<Self, CaptureError> {
        if !GraphicsCaptureSession::IsSupported().map_err(os("IsSupported"))? {
            return Err(CaptureError::Os(
                "Windows Graphics Capture is not available".into(),
            ));
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
        Ok(Self {
            device,
            context,
            winrt,
            factory,
            adapter,
        })
    }
}

/// One frozen monitor: its window and swap chain, and the texture the app reads.
struct Screen {
    window: HWND,
    swap_chain: Option<IDXGISwapChain1>,
    _texture: ID3D11Texture2D,
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
    static MADE_ON: RefCell<Option<(ID3D11DeviceContext, IDXGIDevice3)>> =
        const { RefCell::new(None) };
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
    // The driver keeps the memory of the textures and swap chains it freed (over 100 MB for a
    // 4K screen) for later ones: given back now, at about a millisecond on the next freeze.
    if any && let Some((context, device)) = MADE_ON.with(|m| m.borrow_mut().take()) {
        // SAFETY: plain calls on this thread's device; nothing of it is bound any more.
        unsafe {
            context.ClearState();
            device.Trim();
        }
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
    thread_local! {
        static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if REGISTERED.with(std::cell::Cell::get) {
        return Ok(());
    }
    // SAFETY: a static class name and a window procedure with the required signature.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(os("GetModuleHandleW"))?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(CaptureError::Os("RegisterClassExW failed".into()));
        }
    }
    REGISTERED.with(|r| r.set(true));
    Ok(())
}

fn set_bool_attribute(
    hwnd: HWND,
    attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE,
    on: bool,
) {
    let value = BOOL::from(on);
    // SAFETY: `value` is a live BOOL, the size these attributes want.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            (&raw const value).cast(),
            size_of::<BOOL>() as u32,
        )
    };
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

fn start(gpu: &Gpu, monitor: &MonitorInfo) -> Result<Pending, CaptureError> {
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
    let token = pool
        .FrameArrived(&frame_handler(tx))
        .map_err(os("FrameArrived"))?;
    let session = pool
        .CreateCaptureSession(&item)
        .map_err(os("CreateCaptureSession"))?;
    let pending = Pending {
        pool,
        session,
        token,
        arrived,
    };
    // The cursor and border switches need recent Windows builds; ignore their absence.
    let _ = pending.session.SetIsCursorCaptureEnabled(false);
    let _ = pending.session.SetIsBorderRequired(false);
    pending.session.StartCapture().map_err(os("StartCapture"))?;
    Ok(pending)
}

/// Waits for the first frame and copies it into a new shared texture.
fn finish(
    gpu: &Gpu,
    pending: &Pending,
    deadline: Instant,
) -> Result<ID3D11Texture2D, CaptureError> {
    pending
        .arrived
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| CaptureError::Timeout)?;
    let frame = pending
        .pool
        .TryGetNextFrame()
        .map_err(os("TryGetNextFrame"))?;
    let surface = frame.Surface().map_err(os("Surface"))?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(os("surface access"))?;
    // SAFETY: the surface is backed by a D3D11 texture on our device.
    let source: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(os("GetInterface"))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { source.GetDesc(&mut desc) };
    desc.MipLevels = 1;
    desc.ArraySize = 1;
    desc.SampleDesc = DXGI_SAMPLE_DESC {
        Count: 1,
        Quality: 0,
    };
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
    desc.CPUAccessFlags = 0;
    desc.MiscFlags = SHARED_FLAGS;
    let mut texture = None;
    // SAFETY: a plain texture description; the out-pointer is valid.
    unsafe {
        gpu.device
            .CreateTexture2D(&desc, None, Some(&raw mut texture))
    }
    .map_err(os("CreateTexture2D"))?;
    let texture = texture.ok_or_else(|| CaptureError::Os("no shared texture".into()))?;
    // SAFETY: same device, size and format.
    unsafe { gpu.context.CopyResource(&texture, &source) };
    let _ = frame.Close();
    Ok(texture)
}

/// A hidden (cloaked) top-most window over `monitor`, showing `texture`.
///
/// The window must stay an ordinary window for the compositor. One that covers a monitor
/// exactly with a flip-model swap chain is taken for a full-screen game: the screen switches to
/// "independent flip" (a flash, and variable refresh that follows our single frame). So an SDR
/// screen uses a "blt" swap chain, which the compositor always composes, and the window is one
/// pixel taller than the monitor when no monitor lies below. HDR content needs the flip model.
fn show_screen(
    gpu: &Gpu,
    monitor: &MonitorInfo,
    monitors: &[MonitorInfo],
    texture: ID3D11Texture2D,
) -> Result<(Screen, FrozenScreen), CaptureError> {
    let r = monitor.rect;
    let hdr = monitor.hdr.is_some();
    let bottom = r.y + r.height as i32;
    let below_is_free = !monitors.iter().any(|m| {
        m.rect.y <= bottom
            && bottom < m.rect.y + m.rect.height as i32
            && m.rect.x < r.x + r.width as i32
            && r.x < m.rect.x + m.rect.width as i32
    });
    let extra = u32::from(below_is_free);
    let ex_style = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    let ex_style = if hdr {
        ex_style | WS_EX_NOREDIRECTIONBITMAP
    } else {
        ex_style
    };
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
            Some(
                GetModuleHandleW(None)
                    .map_err(os("GetModuleHandleW"))?
                    .into(),
            ),
            None,
        )
    }
    .map_err(os("CreateWindowExW"))?;
    set_bool_attribute(window, DWMWA_TRANSITIONS_FORCEDISABLED, true);
    set_bool_attribute(window, DWMWA_CLOAK, true);
    // SAFETY: our own window. Kept out of captures, so a capture never sees a frozen screen.
    let _ = unsafe { SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE) };
    let mut screen = Screen {
        window,
        swap_chain: None,
        _texture: texture.clone(),
        shared: None,
    };
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { texture.GetDesc(&mut desc) };
    let chain_desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: desc.Width,
        Height: desc.Height + extra,
        Format: if hdr {
            DXGI_FORMAT_R16G16B16A16_FLOAT
        } else {
            DXGI_FORMAT_B8G8R8A8_UNORM
        },
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: if hdr { 2 } else { 1 },
        Scaling: if hdr {
            DXGI_SCALING_NONE
        } else {
            DXGI_SCALING_STRETCH
        },
        SwapEffect: if hdr {
            DXGI_SWAP_EFFECT_FLIP_DISCARD
        } else {
            DXGI_SWAP_EFFECT_DISCARD
        },
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        ..Default::default()
    };
    // SAFETY: a live device and window; the description outlives the call.
    let chain = unsafe {
        gpu.factory
            .CreateSwapChainForHwnd(&gpu.device, window, &chain_desc, None, None)
    }
    .map_err(os("CreateSwapChainForHwnd"))?;
    if hdr {
        // Linear BT.709 floats, 1.0 = 80 nits: what the capture gave, shown as is.
        if let Ok(chain3) = chain.cast::<IDXGISwapChain3>() {
            // SAFETY: a plain call on a live swap chain.
            let _ = unsafe { chain3.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709) };
        }
    }
    // SAFETY: the back buffer has the texture's format and holds it (plus the extra row).
    // Shown (still cloaked) first: a hidden window's frames are dropped.
    unsafe {
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        let back: ID3D11Texture2D = chain.GetBuffer(0).map_err(os("GetBuffer"))?;
        gpu.context
            .CopySubresourceRegion(&back, 0, 0, 0, 0, &texture, 0, None);
        chain
            .Present(0, DXGI_PRESENT(0))
            .ok()
            .map_err(os("Present"))?;
    }
    screen.swap_chain = Some(chain);
    let id = next_shared_id();
    screen.shared = Some(share(&texture, id)?);
    let shared = FrozenScreen {
        x: r.x,
        y: r.y,
        width: desc.Width,
        height: desc.Height,
        texture: id,
        hdr,
        window: window.0 as usize as u64,
    };
    Ok((screen, shared))
}

/// Freezes every monitor. Must run on a thread with a message loop (the windows live there).
/// The screens go on [`thaw`], on [`THAW_MESSAGE`] or after ten seconds.
pub fn freeze(gpu: &Gpu, monitors: &[MonitorInfo]) -> Result<Frozen, CaptureError> {
    thaw();
    register_class()?;
    // Every capture starts before any is waited for: they run side by side.
    let pending = monitors
        .iter()
        .map(|m| start(gpu, m))
        .collect::<Result<Vec<_>, _>>()?;
    let deadline = Instant::now() + FRAME_TIMEOUT;
    let textures = pending
        .iter()
        .map(|p| finish(gpu, p, deadline))
        .collect::<Result<Vec<_>, _>>()?;
    drop(pending);

    let mut screens = Vec::with_capacity(monitors.len());
    let mut shared = Vec::with_capacity(monitors.len());
    for (monitor, texture) in monitors.iter().zip(textures) {
        let (screen, frozen) = show_screen(gpu, monitor, monitors, texture)?;
        screens.push(screen);
        shared.push(frozen);
    }
    // SAFETY: plain calls. The copies reach the GPU now; once the compositor has taken the
    // presented frames, the windows appear all at once with their content.
    unsafe {
        gpu.context.Flush();
        let _ = DwmFlush();
    }
    for s in &screens {
        set_bool_attribute(s.window, DWMWA_CLOAK, false);
    }
    if let Some(first) = screens.first() {
        // SAFETY: a timer on our own window; its WM_TIMER thaws.
        unsafe { SetTimer(Some(first.window), 1, SAFETY_TIMEOUT_MS, None) };
    }
    ACTIVE.with(|a| *a.borrow_mut() = screens);
    if let Ok(device) = gpu.device.cast::<IDXGIDevice3>() {
        MADE_ON.with(|m| *m.borrow_mut() = Some((gpu.context.clone(), device)));
    }
    Ok(Frozen {
        adapter: gpu.adapter,
        screens: shared,
    })
}

/// The rows of a frozen HDR screen (RGBA half floats, `stride` bytes apart, as [`read_with`]
/// hands them) as scRGB.
pub fn scrgb(screen: &FrozenScreen, rows: &[u8], stride: usize) -> HdrFrame {
    let (w, h) = (screen.width, screen.height);
    HdrFrame {
        width: w,
        height: h,
        rgba: halves_to_f32(rows, stride, w as usize, h as usize),
    }
}

/// The app's device on the daemon's adapter.
struct Reader {
    adapter: i64,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

// SAFETY: a Direct3D 11 device is free-threaded; the immediate context is used by one thread at
// a time (made on the warming thread, then used by the thread that reads the screens).
unsafe impl Send for Reader {}

/// The reader, kept for the next captures of the process.
static READER: Mutex<Option<Reader>> = Mutex::new(None);

/// The reader being made in the background (see [`warm`]).
static WARMING: Mutex<Option<JoinHandle<Result<Reader, CaptureError>>>> = Mutex::new(None);

/// Starts making the device that reads the frozen screens of `frozen`, while the app starts.
pub fn warm(frozen: &Frozen) {
    let adapter = frozen.adapter;
    let made = std::thread::Builder::new()
        .name("frozen-warm".into())
        .spawn(move || make_reader(adapter));
    if let (Ok(handle), Ok(mut warming)) = (made, WARMING.lock()) {
        *warming = Some(handle);
    }
}

fn reader(adapter: i64) -> Result<(ID3D11Device, ID3D11DeviceContext), CaptureError> {
    let mut kept = READER
        .lock()
        .map_err(|_| CaptureError::Os("reader lock".into()))?;
    // SAFETY: plain call.
    let removed = |r: &Reader| unsafe { r.device.GetDeviceRemovedReason() }.is_err();
    if kept
        .as_ref()
        .is_none_or(|r| r.adapter != adapter || removed(r))
    {
        let warming = WARMING.lock().ok().and_then(|mut w| w.take());
        *kept = Some(match warming.map(JoinHandle::join) {
            Some(Ok(Ok(made))) if made.adapter == adapter => made,
            _ => make_reader(adapter)?,
        });
    }
    let r = kept
        .as_ref()
        .ok_or_else(|| CaptureError::Os("no reader".into()))?;
    Ok((r.device.clone(), r.context.clone()))
}

/// The device that reads the frozen screens of `adapter` (made if needed): the editor draws
/// with it too, rather than making one more.
pub fn device(adapter: i64) -> Option<ID3D11Device> {
    reader(adapter).ok().map(|(device, _)| device)
}

fn make_reader(adapter: i64) -> Result<Reader, CaptureError> {
    let luid = LUID {
        LowPart: adapter as u32,
        HighPart: (adapter >> 32) as i32,
    };
    // SAFETY: plain factory calls.
    let dxgi_adapter: IDXGIAdapter = unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory1().map_err(os("CreateDXGIFactory1"))?;
        factory.EnumAdapterByLuid(luid)
    }
    .map_err(os("EnumAdapterByLuid"))?;
    let (device, context) = d3d_device(Some(&dxgi_adapter))?;
    Ok(Reader {
        adapter,
        device,
        context,
    })
}

/// How the frozen textures are created: shared through a named NT handle, which has an access
/// control list (a legacy shared handle is a bare number any process can try).
const SHARED_FLAGS: u32 =
    (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32;

/// Only this user and the system may open a frozen screen, and not a process of a lower
/// integrity level (a sandboxed one).
const SHARED_SDDL: windows::core::PCWSTR = w!("D:P(A;;GA;;;OW)(A;;GA;;;SY)S:(ML;;NWNRNX;;;ME)");

/// A name no other frozen texture has: this process and a counter.
fn next_shared_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    (u64::from(std::process::id()) << 32)
        | u64::from(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
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
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            SHARED_SDDL,
            SDDL_REVISION_1,
            &raw mut descriptor,
            None,
        )
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
            resource.CreateSharedHandle(
                Some(&raw const attributes),
                DXGI_SHARED_RESOURCE_READ.0,
                &shared_name(id),
            )
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

/// Reads the shared textures (in the app): `each` gets every screen's rows (`stride` bytes
/// apart) while they are mapped, so it can convert them straight to where they go. The GPU
/// work of all screens is queued before the first is waited for.
///
/// An HDR screen for which `sdr` gives its levels is tone-mapped to BGRA on the graphics card;
/// it comes as half floats too with `halves`, or when that tone mapping failed (and always
/// after its BGRA rows).
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
            let card = sdr(screen).filter(|_| screen.hdr).and_then(|info| {
                card_tonemap::stage(&device, &context, &texture, &whole, &info).ok()
            });
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
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: a staging texture with CPU read access; `mapped` is a valid out-pointer.
            unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }
                .map_err(os("Map"))?;
            let stride = mapped.RowPitch as usize;
            // SAFETY: a mapped texture exposes `RowPitch * Height` readable bytes until `Unmap`.
            let data = unsafe {
                std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>(),
                    stride * screen.height as usize,
                )
            };
            each(screen, rows, data, stride);
            // SAFETY: matches the successful Map above.
            unsafe { context.Unmap(&staging, 0) };
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
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.MiscFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    // SAFETY: a staging copy of an existing texture; the out-pointer is valid.
    unsafe { device.CreateTexture2D(&desc, None, Some(&raw mut staging)) }
        .map_err(os("CreateTexture2D"))?;
    let staging = staging.ok_or_else(|| CaptureError::Os("no staging texture".into()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D11::D3D11_SUBRESOURCE_DATA;

    /// A texture shared by one device is read by another, as the app reads the daemon's.
    #[test]
    fn shared_textures_are_read_back() {
        let Ok(gpu) = Gpu::new() else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let (w, h) = (3u32, 2u32);
        let pixels: Vec<u8> = (0..w * h * 4).map(|i| i as u8).collect();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: SHARED_FLAGS,
        };
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: w * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        // SAFETY: `init` points at `pixels`, which outlives the call; out-pointer is valid.
        unsafe {
            gpu.device
                .CreateTexture2D(&desc, Some(&init), Some(&raw mut texture))
                .unwrap();
            gpu.context.Flush();
        }
        let texture = texture.unwrap();
        let id = next_shared_id();
        let handle = share(&texture, id).unwrap();
        let frozen = Frozen {
            adapter: gpu.adapter,
            screens: vec![FrozenScreen {
                x: 0,
                y: 0,
                width: w,
                height: h,
                texture: id,
                hdr: false,
                window: 0,
            }],
        };
        let mut read: Vec<u8> = Vec::new();
        read_with(
            &frozen,
            |_| None,
            false,
            |_, kind, rows, stride| {
                assert_eq!(kind, Rows::Bgra);
                read.extend(
                    (0..h as usize).flat_map(|y| &rows[y * stride..y * stride + w as usize * 4]),
                )
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
        let screen = FrozenScreen {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            texture: 0,
            hdr: true,
            window: 0,
        };
        assert_eq!(scrgb(&screen, &rows, 16).rgba, vec![1.0, 0.5, 0.0, 1.0]);
    }
}
