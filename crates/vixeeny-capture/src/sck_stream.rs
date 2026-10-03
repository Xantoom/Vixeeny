// SPDX-License-Identifier: GPL-3.0-or-later
//! ScreenCaptureKit video stream (macOS 13+): BGRA frames on the host clock, the same shape as
//! the Windows `VideoStream`. Frames are copied out of the IOSurface-backed pixel buffer; the
//! GPU path (IOSurface → VideoToolbox without a copy) is a later optimisation.

use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput,
    SCStreamOutputType,
};
use vixeeny_platform::MonitorInfo;

use crate::{CaptureError, CpuFrame};

/// `kCVPixelFormatType_32BGRA`.
const PIXEL_FORMAT_BGRA: u32 = 0x4247_5241;
const START_TIMEOUT: Duration = Duration::from_secs(5);

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

struct Ivars {
    tx: SyncSender<Msg>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "VixeenyStreamOutput"]
    #[ivars = Ivars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(
            &self,
            _stream: &SCStream,
            buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            if let Some(frame) = frame_of(buffer) {
                // The encoder is behind: this frame is dropped, the stream goes on.
                let _ = self.ivars().tx.try_send(Msg::Frame(frame));
            }
        }
    }

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let _ = self
                .ivars()
                .tx
                .try_send(Msg::Stopped(error.localizedDescription().to_string()));
        }
    }
);

impl Output {
    fn new(tx: SyncSender<Msg>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { tx });
        // SAFETY: the superclass initialiser of a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// Seconds of a `CMTime` as nanoseconds.
fn nanoseconds(t: CMTime) -> i64 {
    if t.timescale <= 0 {
        return 0;
    }
    (i128::from(t.value) * 1_000_000_000 / i128::from(t.timescale)) as i64
}

/// The BGRA frame of a sample buffer; `None` for the bookkeeping buffers (no picture: nothing
/// changed on screen).
fn frame_of(buffer: &CMSampleBuffer) -> Option<StreamFrame> {
    // SAFETY: reading properties of a live sample buffer, valid for this callback.
    let (image, pts) = unsafe { (buffer.image_buffer()?, buffer.presentation_time_stamp()) };
    let (width, height) = (
        CVPixelBufferGetWidth(&image),
        CVPixelBufferGetHeight(&image),
    );
    // SAFETY: locking a live pixel buffer; unlocked below on every path that gets past this.
    if unsafe { CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) } != 0 {
        return None;
    }
    let stride = CVPixelBufferGetBytesPerRow(&image);
    let base = CVPixelBufferGetBaseAddress(&image).cast::<u8>();
    let frame = NonNull::new(base).and_then(|base| {
        // SAFETY: the buffer is locked, `stride × height` bytes are readable from its base.
        let bytes = unsafe { std::slice::from_raw_parts(base.as_ptr(), stride * height) };
        CpuFrame::from_raw(width as u32, height as u32, stride, bytes.to_vec()).ok()
    });
    // SAFETY: matches the successful lock above.
    unsafe { CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) };
    Some(StreamFrame {
        time_ns: nanoseconds(pts),
        frame: frame?,
    })
}

/// A running capture; stops when dropped.
pub struct SckVideoStream {
    stream: Retained<SCStream>,
    rx: Receiver<Msg>,
    size: (u32, u32),
    // Kept alive: the stream holds its output weakly.
    _output: Retained<Output>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl SckVideoStream {
    /// Captures `monitor` at up to `fps` images per second.
    pub fn start_monitor(
        monitor: &MonitorInfo,
        fps: u32,
        cursor: bool,
    ) -> Result<Self, CaptureError> {
        let id = monitor.id.0 as u32;
        let content = crate::sck::shareable_content()?;
        // SAFETY: reading the displays of live content.
        let display = unsafe { content.displays() }
            .iter()
            .find(|d| unsafe { d.displayID() } == id)
            .ok_or(CaptureError::UnknownMonitor)?;
        // SAFETY: object construction and property setters.
        let (filter, config) = unsafe {
            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            );
            let config = SCStreamConfiguration::new();
            config.setWidth(monitor.rect.width as usize);
            config.setHeight(monitor.rect.height as usize);
            config.setShowsCursor(cursor);
            config.setPixelFormat(PIXEL_FORMAT_BGRA);
            config.setQueueDepth(5);
            config.setMinimumFrameInterval(CMTime {
                value: 1,
                timescale: fps.clamp(1, 240) as i32,
                flags: objc2_core_media::CMTimeFlags::Valid,
                epoch: 0,
            });
            (filter, config)
        };
        let (tx, rx) = sync_channel(4);
        let output = Output::new(tx);
        let queue = DispatchQueue::new("vixeeny.capture", None);
        // SAFETY: the delegate and output outlive the stream (kept in `self`).
        let stream = unsafe {
            let stream = SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            );
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|e| CaptureError::Os(e.localizedDescription().to_string()))?;
            stream
        };
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let handler = RcBlock::new(move |error: *mut NSError| {
            let _ = started_tx.send(crate::sck::error_text(error));
        });
        // SAFETY: the block lives until the call returns and is copied by the framework.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&handler)) };
        match started_rx.recv_timeout(START_TIMEOUT) {
            Ok(None) => {}
            Ok(Some(e)) => return Err(CaptureError::Os(e)),
            Err(_) => return Err(CaptureError::Timeout),
        }
        Ok(Self {
            stream,
            rx,
            size: (monitor.rect.width, monitor.rect.height),
            _output: output,
            _queue: queue,
        })
    }

    pub fn source_size(&self) -> (u32, u32) {
        self.size
    }

    /// The next frame; `Ok(None)` when none came within `timeout` (nothing changed on screen).
    pub fn recv(&self, timeout: Duration) -> Result<Option<StreamFrame>, CaptureError> {
        match self.rx.recv_timeout(timeout) {
            Ok(Msg::Frame(frame)) => Ok(Some(frame)),
            Ok(Msg::Stopped(why)) => Err(CaptureError::Os(format!("capture stopped: {why}"))),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Os("capture stopped".into())),
        }
    }
}

impl Drop for SckVideoStream {
    fn drop(&mut self) {
        // SAFETY: stopping a stream we started; the completion is not awaited.
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
    }
}
