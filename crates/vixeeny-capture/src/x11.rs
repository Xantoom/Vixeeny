// SPDX-License-Identifier: GPL-3.0-or-later
//! X11 capture (native sessions and XWayland): `GetImage` on the root window. There is no
//! permission to ask for; what is on the screen, composited, is what comes back. The cursor is
//! not part of the image (the option is ignored).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use vixeeny_platform::{MonitorInfo, PhysicalRect, WindowId};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use crate::{CaptureError, CpuFrame, StillBackend};

fn os_error(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Os(e.to_string())
}

struct Display {
    conn: RustConnection,
    root: Window,
    /// Bytes per pixel of the root's image format.
    bytes: usize,
}

impl Display {
    fn open() -> Result<Self, CaptureError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(os_error)?;
        let screen = &conn.setup().roots[screen_num];
        let depth = screen.root_depth;
        let format = conn
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .ok_or_else(|| CaptureError::Os("no pixmap format for the root depth".into()))?;
        if format.bits_per_pixel != 32 {
            return Err(CaptureError::Os(format!(
                "unsupported pixel format ({} bits per pixel)",
                format.bits_per_pixel
            )));
        }
        Ok(Self {
            root: screen.root,
            conn,
            bytes: 4,
        })
    }

    /// `rect` of the root window as BGRA with an opaque alpha.
    fn grab(&self, rect: &PhysicalRect) -> Result<CpuFrame, CaptureError> {
        let screen = &self.conn.setup().roots[0];
        let (sw, sh) = (
            i32::from(screen.width_in_pixels),
            i32::from(screen.height_in_pixels),
        );
        // The part of the rectangle that is on the screen.
        let x0 = rect.x.clamp(0, sw);
        let y0 = rect.y.clamp(0, sh);
        let x1 = (rect.x + rect.width as i32).clamp(0, sw);
        let y1 = (rect.y + rect.height as i32).clamp(0, sh);
        if x1 <= x0 || y1 <= y0 {
            return Err(CaptureError::InvalidRegion);
        }
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                x0 as i16,
                y0 as i16,
                w as u16,
                h as u16,
                !0,
            )
            .map_err(os_error)?
            .reply()
            .map_err(os_error)?;
        let stride = w as usize * self.bytes;
        let mut data = reply.data;
        if data.len() < stride * h as usize {
            return Err(CaptureError::Os("short image".into()));
        }
        // Little-endian X servers hand out B, G, R, X: the unused byte becomes opaque.
        for px in data.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        CpuFrame::from_raw(w, h, stride, data)
    }
}

pub struct X11Backend {
    display: Display,
}

impl X11Backend {
    pub fn new() -> Result<Self, CaptureError> {
        Ok(Self {
            display: Display::open()?,
        })
    }
}

impl StillBackend for X11Backend {
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        _cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        self.display.grab(&monitor.rect)
    }

    /// What the window shows on the screen (a covered part shows what covers it).
    fn grab_window(&mut self, window: WindowId, _cursor: bool) -> Result<CpuFrame, CaptureError> {
        let info = vixeeny_platform::window_info(window)
            .map_err(os_error)?
            .ok_or(CaptureError::UnknownMonitor)?;
        self.display.grab(&info.rect)
    }
}

/// A frame and when it was produced (nanoseconds on `vixeeny_platform::monotonic_ns`).
#[derive(Debug)]
pub struct StreamFrame {
    pub time_ns: i64,
    pub frame: CpuFrame,
}

enum Msg {
    Frame(StreamFrame),
    Stopped(String),
}

/// A running capture of one monitor, polled `fps` times a second; stops when dropped.
pub struct X11VideoStream {
    rx: Receiver<Msg>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    size: (u32, u32),
}

fn poll(display: &Display, rect: PhysicalRect, fps: u32, tx: &SyncSender<Msg>, stop: &AtomicBool) {
    let period = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        match display.grab(&rect) {
            Ok(frame) => {
                let time_ns = vixeeny_platform::monotonic_ns();
                // A slow consumer loses frames; it is never blocked on the capture.
                let _ = tx.try_send(Msg::Frame(StreamFrame { time_ns, frame }));
            }
            Err(e) => {
                let _ = tx.send(Msg::Stopped(e.to_string()));
                return;
            }
        }
        next += period;
        match next.checked_duration_since(Instant::now()) {
            Some(wait) => std::thread::sleep(wait),
            // Late: do not try to catch up with a burst.
            None => next = Instant::now(),
        }
    }
}

impl X11VideoStream {
    pub fn start_monitor(
        monitor: &MonitorInfo,
        fps: u32,
        _cursor: bool,
    ) -> Result<Self, CaptureError> {
        let display = Display::open()?;
        let rect = monitor.rect;
        let (tx, rx) = sync_channel(4);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("x11-capture".into())
            .spawn(move || poll(&display, rect, fps, &tx, &flag))
            .map_err(os_error)?;
        Ok(Self {
            rx,
            stop,
            thread: Some(thread),
            size: (monitor.rect.width, monitor.rect.height),
        })
    }

    pub fn source_size(&self) -> (u32, u32) {
        self.size
    }

    /// The next frame; `None` when none arrived within `timeout`.
    pub fn recv(&self, timeout: Duration) -> Result<Option<StreamFrame>, CaptureError> {
        match self.rx.recv_timeout(timeout) {
            Ok(Msg::Frame(f)) => Ok(Some(f)),
            Ok(Msg::Stopped(e)) => Err(CaptureError::Os(e)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Os("capture ended".into())),
        }
    }
}

impl Drop for X11VideoStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
