// SPDX-License-Identifier: GPL-3.0-or-later
//! A monitor's picture through Desktop Duplication: what it shows now, in a few milliseconds
//! (two 4K screens: 4 to 5 ms), where Windows Graphics Capture waits for its session and the
//! next composed frame (about 36 ms each). The same pixels, and windows excluded from capture
//! stay out of it too.
//!
//! It leaves out the pointer, and is made only for an unrotated monitor of the device's
//! adapter, in the format the monitor composes in; the caller captures through Windows Graphics
//! Capture otherwise, and whenever this fails (a secure desktop, a UAC prompt, too many
//! duplications of the screen at once).

use std::time::{Duration, Instant};

use vixeeny_platform::MonitorInfo;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice, IDXGIOutput5, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

/// How long the first picture may take before Windows Graphics Capture is used instead.
const WAIT: Duration = Duration::from_millis(100);

/// A picture held by its duplication until dropped.
pub(crate) struct Frame {
    duplication: IDXGIOutputDuplication,
    pub texture: ID3D11Texture2D,
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: the frame acquired by this duplication, released once.
        let _ = unsafe { self.duplication.ReleaseFrame() };
    }
}

/// `monitor`'s picture on `device`, 8-bit BGRA for an SDR monitor and scRGB half floats for an
/// HDR one (`hdr`: which is asked for); `None` when Desktop Duplication cannot give it.
pub(crate) fn frame(device: &ID3D11Device, monitor: &MonitorInfo, hdr: bool) -> Option<Frame> {
    if hdr != monitor.hdr.is_some() {
        return None;
    }
    let format: DXGI_FORMAT = if hdr {
        DXGI_FORMAT_R16G16B16A16_FLOAT
    } else {
        DXGI_FORMAT_B8G8R8A8_UNORM
    };
    let r = monitor.rect;
    // SAFETY: plain DXGI queries and calls on live objects; the frame info and resource are
    // valid out-pointers.
    unsafe {
        let adapter = device.cast::<IDXGIDevice>().ok()?.GetAdapter().ok()?;
        let output = (0..).map_while(|i| adapter.EnumOutputs(i).ok()).find(|o| {
            o.GetDesc().is_ok_and(|d| {
                let c = d.DesktopCoordinates;
                d.AttachedToDesktop.as_bool()
                    && (c.left, c.top) == (r.x, r.y)
                    && (c.right - c.left, c.bottom - c.top) == (r.width as i32, r.height as i32)
                    && matches!(
                        d.Rotation,
                        DXGI_MODE_ROTATION_IDENTITY | DXGI_MODE_ROTATION_UNSPECIFIED
                    )
            })
        })?;
        let duplication = output
            .cast::<IDXGIOutput5>()
            .ok()?
            .DuplicateOutput1(device, 0, &[format])
            .ok()?;
        let desc = duplication.GetDesc();
        if desc.ModeDesc.Format != format || desc.DesktopImageInSystemMemory.as_bool() {
            return None;
        }
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            duplication
                .AcquireNextFrame(left.as_millis() as u32, &raw mut info, &raw mut resource)
                .ok()?;
            // The first frames may carry only the pointer: no picture yet.
            if info.LastPresentTime != 0
                && let Some(texture) = resource.and_then(|r| r.cast::<ID3D11Texture2D>().ok())
            {
                return Some(Frame {
                    duplication,
                    texture,
                });
            }
            let _ = duplication.ReleaseFrame();
            if left.is_zero() {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StillBackend;

    /// A duplicated screen is the picture Windows Graphics Capture gives (the pointer left out
    /// of both).
    #[test]
    fn a_duplicated_screen_is_the_captured_one() {
        vixeeny_platform::ensure_dpi_aware();
        let (Ok(mut backend), Ok(monitors)) =
            (crate::WgcBackend::new(), vixeeny_platform::monitors())
        else {
            vixeeny_common::skip_test("no Direct3D device");
            return;
        };
        let Some(monitor) = monitors.iter().find(|m| m.hdr.is_none()) else {
            vixeeny_common::skip_test("no SDR monitor");
            return;
        };
        let device = backend.d3d_device().unwrap();
        let started = Instant::now();
        let Some(frame) = frame(&device, monitor, false) else {
            vixeeny_common::skip_test("no Desktop Duplication here");
            return;
        };
        eprintln!("duplicated in {:?}", started.elapsed());
        // SAFETY: a plain query on a live device.
        let context = unsafe { device.GetImmediateContext() }.unwrap();
        let mut desc = Default::default();
        // SAFETY: `desc` is a valid out-pointer.
        unsafe { frame.texture.GetDesc(&mut desc) };
        assert_eq!(
            (desc.Width, desc.Height),
            (monitor.rect.width, monitor.rect.height)
        );
        let duplicated = crate::wgc::read_back(&device, &context, &frame.texture).unwrap();
        drop(frame);
        // With the pointer asked, the capture goes through Windows Graphics Capture.
        let captured = backend.grab_monitor(monitor, true).unwrap();
        // Colours only: the alpha of a duplicated screen is not always opaque.
        let (mut same, mut lit, mut n) = (0u64, 0u64, 0u64);
        for y in (0..desc.Height as usize).step_by(5) {
            for x in (0..desc.Width as usize * 4)
                .step_by(11)
                .filter(|x| x % 4 != 3)
            {
                let a = duplicated.data[y * duplicated.stride + x];
                let b = captured.data[y * captured.stride + x];
                same += u64::from(a.abs_diff(b) <= 2);
                lit += u64::from(a > 0);
                n += 1;
            }
        }
        eprintln!("{:.1} % the same", same as f64 * 100.0 / n as f64);
        assert!(lit > 0, "a black picture");
        // Most of it: the pointer, a video or anything animated moves meanwhile.
        assert!(same * 2 > n, "{same} of {n} samples the same");
    }
}
