// SPDX-License-Identifier: GPL-3.0-or-later
//! ScreenCaptureKit backend for single images (macOS 14+, `SCScreenshotManager`). The screen
//! recording permission is checked first: without it the call would hang or return a wallpaper.
//!
//! Not done yet: macOS 13 (no screenshot manager there; needs a one-frame
//! `SCStream`), HDR, excluding the app's own windows.

use std::ptr::NonNull;
use std::sync::mpsc::channel;
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2_core_graphics::{CGDataProvider, CGImage};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};
use vixeeny_platform::{MonitorInfo, WindowId};

use crate::{CaptureError, CpuFrame, StillBackend};

/// `kCVPixelFormatType_32BGRA`.
const PIXEL_FORMAT_BGRA: u32 = 0x4247_5241;
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct SckBackend;

impl SckBackend {
    pub fn new() -> Result<Self, CaptureError> {
        if !vixeeny_platform::screen_capture_allowed() {
            // Shows the system prompt the first time; the user must then restart the app.
            vixeeny_platform::request_screen_capture_access();
            return Err(CaptureError::Os(
                "screen recording is not allowed (System Settings → Privacy & Security)".into(),
            ));
        }
        if AnyClass::get(c"SCScreenshotManager").is_none() {
            return Err(CaptureError::Os(
                "ScreenCaptureKit screenshots need macOS 14".into(),
            ));
        }
        Ok(Self)
    }
}

/// The message of an `NSError` a completion handler received (null = none).
fn error_text(error: *mut NSError) -> Option<String> {
    // SAFETY: a completion handler's error is null or a valid object for the call's duration.
    unsafe { Retained::retain(error) }.map(|e| e.localizedDescription().to_string())
}

/// What can be captured right now (displays, windows).
fn shareable_content() -> Result<Retained<SCShareableContent>, CaptureError> {
    let (tx, rx) = channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: the content pointer is null or valid for the call; it is retained here.
            let content = unsafe { Retained::retain(content) };
            let _ =
                tx.send(content.ok_or_else(|| {
                    error_text(error).unwrap_or_else(|| "nothing to capture".into())
                }));
        },
    );
    // SAFETY: the block lives until the call returns and is copied by the framework.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    rx.recv_timeout(TIMEOUT)
        .map_err(|_| CaptureError::Timeout)?
        .map_err(CaptureError::Os)
}

/// Takes the picture described by `filter`, `width`×`height` pixels.
fn screenshot(
    filter: &SCContentFilter,
    width: usize,
    height: usize,
    cursor: bool,
) -> Result<CpuFrame, CaptureError> {
    // SAFETY: plain Objective-C object construction and property setters.
    let config = unsafe {
        let config = SCStreamConfiguration::new();
        config.setWidth(width);
        config.setHeight(height);
        config.setShowsCursor(cursor);
        config.setPixelFormat(PIXEL_FORMAT_BGRA);
        config
    };
    let (tx, rx) = channel();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = match NonNull::new(image) {
            // SAFETY: the image is valid during the callback; the frame is copied out.
            Some(image) => frame_of(unsafe { image.as_ref() }),
            None => Err(error_text(error).unwrap_or_else(|| "no image".to_owned())),
        };
        let _ = tx.send(result);
    });
    // SAFETY: filter, configuration and block outlive the call.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            filter,
            &config,
            Some(&handler),
        );
    }
    rx.recv_timeout(TIMEOUT)
        .map_err(|_| CaptureError::Timeout)?
        .map_err(CaptureError::Os)
}

/// Packs a BGRA `CGImage` into a [`CpuFrame`].
fn frame_of(image: &CGImage) -> Result<CpuFrame, String> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    let stride = CGImage::bytes_per_row(Some(image));
    if CGImage::bits_per_pixel(Some(image)) != 32 {
        return Err("unexpected pixel format".into());
    }
    let provider = CGImage::data_provider(Some(image)).ok_or("no pixel data")?;
    let data = CGDataProvider::data(Some(&provider)).ok_or("no pixel data")?;
    // SAFETY: the data is owned here and not mutated while it is copied.
    let bytes = unsafe { data.as_bytes_unchecked() }.to_vec();
    CpuFrame::from_raw(width as u32, height as u32, stride, bytes).map_err(|e| e.to_string())
}

impl StillBackend for SckBackend {
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        let id = monitor.id.0 as u32;
        let content = shareable_content()?;
        // SAFETY: reading the displays of live content.
        let display = unsafe { content.displays() }
            .iter()
            .find(|d| unsafe { d.displayID() } == id)
            .ok_or(CaptureError::UnknownMonitor)?;
        // SAFETY: object construction.
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };
        screenshot(
            &filter,
            monitor.rect.width as usize,
            monitor.rect.height as usize,
            cursor,
        )
    }

    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError> {
        let content = shareable_content()?;
        // SAFETY: reading the windows of live content.
        let found = unsafe { content.windows() }
            .iter()
            .find(|w| unsafe { w.windowID() } == window.0 as u32)
            .ok_or(CaptureError::UnknownMonitor)?;
        // SAFETY: object construction, then reading what the filter says about its content.
        let (filter, width, height) = unsafe {
            let filter =
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &found);
            let rect = filter.contentRect();
            let scale = f64::from(filter.pointPixelScale());
            let size = |points: f64| (points * scale).round().max(1.0) as usize;
            (filter, size(rect.size.width), size(rect.size.height))
        };
        screenshot(&filter, width, height, cursor)
    }
}
