// SPDX-License-Identifier: GPL-3.0-or-later
//! Wayland stills through the `org.freedesktop.portal.Screenshot` portal: the desktop asks (the
//! first time) and hands over one image of every monitor. The monitors then are cut out of it.
//!
//! The compositor decides what the image contains: its pixels may be scaled compared to the
//! monitors' logical rectangles (fractional scaling), so the cut-outs follow the ratio.

use vixeeny_platform::{MonitorInfo, PhysicalRect, WindowId, virtual_bounds};

use crate::{CaptureError, CpuFrame, StillBackend};

/// Whether the session is Wayland (the X server, if any, cannot read other applications' windows).
pub fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Decodes a PNG into BGRA (opaque).
pub fn decode_png(bytes: &[u8]) -> Result<CpuFrame, CaptureError> {
    let os = |e: &dyn std::fmt::Display| CaptureError::Os(format!("screenshot image: {e}"));
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| os(&e))?;
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| os(&"too large"))?
    ];
    let info = reader.next_frame(&mut buffer).map_err(|e| os(&e))?;
    let (w, h) = (info.width, info.height);
    let channels = info.color_type.samples();
    let mut data = Vec::with_capacity(w as usize * h as usize * 4);
    for px in buffer[..info.buffer_size()].chunks_exact(channels) {
        let (r, g, b) = match channels {
            1 | 2 => (px[0], px[0], px[0]),
            _ => (px[0], px[1], px[2]),
        };
        data.extend_from_slice(&[b, g, r, 255]);
    }
    CpuFrame::from_raw(w, h, w as usize * 4, data)
}

/// The rectangle of `rect` (desktop coordinates) inside an image of `image` pixels that shows the
/// desktop area `bounds`: the image may be scaled.
pub fn image_rect(rect: &PhysicalRect, bounds: &PhysicalRect, image: (u32, u32)) -> PhysicalRect {
    let sx = f64::from(image.0) / f64::from(bounds.width.max(1));
    let sy = f64::from(image.1) / f64::from(bounds.height.max(1));
    let x = (f64::from(rect.x - bounds.x) * sx).round() as i32;
    let y = (f64::from(rect.y - bounds.y) * sy).round() as i32;
    let w = (f64::from(rect.width) * sx).round().max(1.0) as u32;
    let h = (f64::from(rect.height) * sy).round().max(1.0) as u32;
    PhysicalRect::new(x, y, w, h)
}

async fn screenshot_uri() -> Result<String, CaptureError> {
    let os = |e: &dyn std::fmt::Display| CaptureError::Os(format!("screenshot portal: {e}"));
    let response = ashpd::desktop::screenshot::Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await
        .map_err(|e| os(&e))?
        .response()
        .map_err(|e| os(&e))?;
    Ok(response.uri().as_str().to_owned())
}

/// The path of a `file:///…` URI (percent-escapes decoded).
fn file_uri_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/path` is not a local file the portal would hand out; `file:///path` is.
    let path = rest.strip_prefix('/').map(|p| format!("/{p}"))?;
    let mut out = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = path.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(std::path::PathBuf::from(String::from_utf8(out).ok()?))
}

/// One image of the whole desktop, from the portal.
fn desktop_image() -> Result<CpuFrame, CaptureError> {
    let uri = async_io::block_on(screenshot_uri())?;
    let path = file_uri_path(&uri)
        .ok_or_else(|| CaptureError::Os(format!("unexpected screenshot location: {uri}")))?;
    let bytes = std::fs::read(&path).map_err(|e| CaptureError::Os(e.to_string()))?;
    // The portal leaves a file in the user's pictures: it was only a transport.
    let _ = std::fs::remove_file(&path);
    decode_png(&bytes)
}

pub struct PortalBackend {
    monitors: Vec<MonitorInfo>,
}

impl PortalBackend {
    pub fn new(monitors: Vec<MonitorInfo>) -> Self {
        Self { monitors }
    }

    fn cut(&self, rect: &PhysicalRect) -> Result<CpuFrame, CaptureError> {
        let bounds =
            virtual_bounds(self.monitors.iter().map(|m| &m.rect)).ok_or(CaptureError::NoMonitor)?;
        let image = desktop_image()?;
        let crop = image_rect(rect, &bounds, (image.width, image.height));
        image.crop(&crop)
    }
}

impl StillBackend for PortalBackend {
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        _cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        self.cut(&monitor.rect)
    }

    fn grab_window(&mut self, window: WindowId, _cursor: bool) -> Result<CpuFrame, CaptureError> {
        // Wayland does not hand out other applications' windows: the cut-out is the screen area
        // the window (an XWayland one) covers.
        let info = vixeeny_platform::window_info(window)
            .map_err(|e| CaptureError::Os(e.to_string()))?
            .ok_or(CaptureError::UnknownMonitor)?;
        self.cut(&info.rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap_or_else(|e| panic!("{e}"));
            let data: Vec<u8> = (0..width * height).flat_map(|_| rgb).collect();
            writer
                .write_image_data(&data)
                .unwrap_or_else(|e| panic!("{e}"));
        }
        out
    }

    #[test]
    fn a_png_becomes_opaque_bgra() {
        let frame = decode_png(&png_of(3, 2, [10, 20, 30])).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((frame.width, frame.height), (3, 2));
        assert_eq!(frame.pixel(2, 1), [30, 20, 10, 255]);
    }

    #[test]
    fn a_file_uri_becomes_a_path() {
        assert_eq!(
            file_uri_path("file:///home/me/Pictures/Screenshot%20from%202026.png"),
            Some(std::path::PathBuf::from(
                "/home/me/Pictures/Screenshot from 2026.png"
            ))
        );
        assert_eq!(file_uri_path("https://example.org/a.png"), None);
        assert_eq!(file_uri_path("file:///bad%2"), None);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn an_unscaled_image_cuts_at_the_monitor_position() {
        let bounds = PhysicalRect::new(0, 0, 3840, 1080);
        let right = PhysicalRect::new(1920, 0, 1920, 1080);
        assert_eq!(image_rect(&right, &bounds, (3840, 1080)), right);
    }

    #[test]
    fn a_scaled_image_cuts_proportionally_and_from_the_origin() {
        // Monitors laid out from (-1920, 0); the image is twice as large as the layout.
        let bounds = PhysicalRect::new(-1920, 0, 3840, 1080);
        let right = PhysicalRect::new(0, 0, 1920, 1080);
        assert_eq!(
            image_rect(&right, &bounds, (7680, 2160)),
            PhysicalRect::new(3840, 0, 3840, 2160)
        );
    }
}

/// The still backend of the running Linux session: the portal on Wayland, X11 otherwise.
pub enum LinuxBackend {
    Portal(PortalBackend),
    X11(Box<crate::X11Backend>),
}

impl LinuxBackend {
    pub fn new(monitors: &[MonitorInfo]) -> Result<Self, CaptureError> {
        if is_wayland_session() {
            Ok(Self::Portal(PortalBackend::new(monitors.to_vec())))
        } else {
            crate::X11Backend::new().map(|b| Self::X11(Box::new(b)))
        }
    }
}

impl StillBackend for LinuxBackend {
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        match self {
            Self::Portal(b) => b.grab_monitor(monitor, cursor),
            Self::X11(b) => b.grab_monitor(monitor, cursor),
        }
    }

    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError> {
        match self {
            Self::Portal(b) => b.grab_window(window, cursor),
            Self::X11(b) => b.grab_window(window, cursor),
        }
    }
}
