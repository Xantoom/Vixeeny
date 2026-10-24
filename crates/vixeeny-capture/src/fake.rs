// SPDX-License-Identifier: GPL-3.0-or-later
//! Test backend: every pixel encodes where it comes from.

use vixeeny_platform::{MonitorId, MonitorInfo, PhysicalRect, WindowId};

use crate::{CaptureError, CpuFrame, HdrFrame, StillBackend};

/// Pixel `(x, y)` of monitor `id` is `[x % 256, y % 256, id, 255]`.
#[derive(Debug, Default)]
pub struct FakeBackend {
    pub grabs: Vec<String>,
    /// The monitor windows are on.
    pub window_monitor: Option<MonitorId>,
}

impl StillBackend for FakeBackend {
    fn grab_monitor(
        &mut self,
        monitor: &MonitorInfo,
        cursor: bool,
    ) -> Result<CpuFrame, CaptureError> {
        self.grabs
            .push(format!("monitor {} cursor={cursor}", monitor.id.0));
        let mut frame = CpuFrame::new(monitor.rect.width, monitor.rect.height);
        for y in 0..frame.height {
            for x in 0..frame.width {
                let i = y as usize * frame.stride + x as usize * 4;
                frame.data[i..i + 4].copy_from_slice(&[x as u8, y as u8, monitor.id.0 as u8, 255]);
            }
        }
        Ok(frame)
    }

    /// Every float is 2.0 (160 nits in scRGB).
    fn grab_monitor_hdr(
        &mut self,
        monitor: &MonitorInfo,
        area: Option<&PhysicalRect>,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        self.grabs
            .push(format!("hdr monitor {} cursor={cursor}", monitor.id.0));
        let pixels = monitor.rect.width as usize * monitor.rect.height as usize;
        let frame = HdrFrame {
            width: monitor.rect.width,
            height: monitor.rect.height,
            rgba: vec![2.0; pixels * 4],
        };
        match area {
            Some(area) => frame.crop(area),
            None => Ok(frame),
        }
    }

    fn grab_window(&mut self, window: WindowId, cursor: bool) -> Result<CpuFrame, CaptureError> {
        self.grabs
            .push(format!("window {} cursor={cursor}", window.0));
        Ok(CpuFrame::new(64, 48))
    }

    /// Every float is 2.0.
    fn grab_window_hdr(
        &mut self,
        window: WindowId,
        cursor: bool,
    ) -> Result<HdrFrame, CaptureError> {
        self.grabs
            .push(format!("hdr window {} cursor={cursor}", window.0));
        Ok(HdrFrame {
            width: 64,
            height: 48,
            rgba: vec![2.0; 64 * 48 * 4],
        })
    }

    fn window_monitor(&self, _window: WindowId) -> Option<MonitorId> {
        self.window_monitor
    }
}

#[cfg(test)]
mod tests {
    use vixeeny_platform::{MonitorId, PhysicalRect};

    use super::*;
    use crate::{CaptureOptions, CaptureTarget, Capturer};

    fn mon(id: u64, x: i32, y: i32, w: u32, h: u32, dpi: u32) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId(id),
            name: format!("DISPLAY{id}"),
            rect: PhysicalRect::new(x, y, w, h),
            work: PhysicalRect::new(x, y, w, h),
            primary: id == 1,
            dpi,
            hdr: None,
        }
    }

    /// a 150 % monitor with a negative-origin 100 % monitor on its left.
    fn capturer() -> Capturer<FakeBackend> {
        let monitors = vec![mon(1, 0, 0, 300, 200, 144), mon(2, -160, 20, 160, 100, 96)];
        Capturer::new(FakeBackend::default(), monitors)
    }

    #[test]
    fn monitor_frame_has_physical_size() {
        let f = capturer()
            .grab(
                &CaptureTarget::Monitor(MonitorId(1)),
                CaptureOptions::default(),
            )
            .unwrap();
        assert_eq!((f.width, f.height), (300, 200));
    }

    #[test]
    fn region_is_relative_to_the_monitor() {
        let target = CaptureTarget::Region {
            monitor: MonitorId(2),
            rect: PhysicalRect::new(10, 5, 20, 10),
        };
        let f = capturer().grab(&target, CaptureOptions::default()).unwrap();
        assert_eq!((f.width, f.height), (20, 10));
        assert_eq!(f.pixel(0, 0), [10, 5, 2, 255]);
        let outside = CaptureTarget::Region {
            monitor: MonitorId(2),
            rect: PhysicalRect::new(150, 0, 20, 10),
        };
        assert!(matches!(
            capturer().grab(&outside, CaptureOptions::default()),
            Err(CaptureError::InvalidRegion)
        ));
    }

    #[test]
    fn all_monitors_are_assembled_on_the_virtual_desktop() {
        let f = capturer()
            .grab(&CaptureTarget::AllMonitors, CaptureOptions::default())
            .unwrap();
        // Bounds: x -160..300, y 0..200.
        assert_eq!((f.width, f.height), (460, 200));
        // Monitor 2 starts at canvas (0, 20); its pixel (3, 4) lands at (3, 24).
        assert_eq!(f.pixel(3, 24), [3, 4, 2, 255]);
        // Monitor 1 starts at canvas x = 160.
        assert_eq!(f.pixel(160 + 7, 9), [7, 9, 1, 255]);
        // Uncovered area stays black.
        assert_eq!(f.pixel(0, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn unknown_monitor() {
        assert!(matches!(
            capturer().grab(
                &CaptureTarget::Monitor(MonitorId(9)),
                CaptureOptions::default()
            ),
            Err(CaptureError::UnknownMonitor)
        ));
    }

    #[test]
    fn hdr_monitors_go_through_the_tone_mapper() {
        use vixeeny_platform::HdrInfo;
        let hdr = HdrInfo {
            sdr_white_nits: 80.0,
            peak_nits: 1000.0,
        };
        let mut monitors = vec![mon(1, 0, 0, 4, 2, 96), mon(2, 4, 0, 4, 2, 96)];
        monitors[1].hdr = Some(hdr);
        let mut c = Capturer::new(FakeBackend::default(), monitors);
        // A fake tone mapper that encodes the headroom it was given into the blue channel.
        let options = CaptureOptions {
            tonemap: Some(|rgba, info| {
                assert!(rgba.iter().all(|v| *v == 2.0));
                [(info.peak_nits / 10.0) as u8, 0, 0, 255].repeat(rgba.len() / 4)
            }),
            ..CaptureOptions::default()
        };
        let frame = c.grab(&CaptureTarget::AllMonitors, options).unwrap();
        assert_eq!((frame.width, frame.height), (8, 2));
        assert_eq!(frame.pixel(0, 0), [0, 0, 1, 255]); // SDR monitor: [x, y, id, 255]
        assert_eq!(frame.pixel(4, 0), [100, 0, 0, 255]); // HDR monitor: tone-mapped
        assert_eq!(
            c.backend.grabs,
            vec!["monitor 1 cursor=false", "hdr monitor 2 cursor=false"]
        );
        // Without a tone mapper the HDR monitor is captured as a plain SDR frame.
        let mut c2 = Capturer::new(
            FakeBackend::default(),
            vec![{
                let mut m = mon(2, 0, 0, 4, 2, 96);
                m.hdr = Some(hdr);
                m
            }],
        );
        c2.grab(
            &CaptureTarget::Monitor(MonitorId(2)),
            CaptureOptions::default(),
        )
        .unwrap();
        assert_eq!(c2.backend.grabs, vec!["monitor 2 cursor=false"]);
    }

    fn hdr(sdr_white_nits: f32, peak_nits: f32) -> vixeeny_platform::HdrInfo {
        vixeeny_platform::HdrInfo {
            sdr_white_nits,
            peak_nits,
        }
    }

    #[test]
    fn hdr_captures_bring_every_screen_to_one_sdr_white() {
        // An SDR monitor, an HDR one at 160 nits SDR white, another at 240 (1000-nit peak).
        let mut monitors = vec![
            mon(1, 0, 0, 2, 1, 96),
            mon(2, 2, 0, 2, 1, 96),
            mon(3, 4, 0, 2, 1, 96),
        ];
        monitors[1].hdr = Some(hdr(160.0, 600.0));
        monitors[2].hdr = Some(hdr(240.0, 1000.0));
        let mut c = Capturer::new(FakeBackend::default(), monitors);
        let all = c
            .grab_hdr(&CaptureTarget::AllMonitors, false)
            .unwrap()
            .unwrap();
        assert_eq!(all.info, hdr(240.0, 1000.0));
        assert_eq!((all.frame.width, all.frame.height), (6, 1));
        let px = |x: usize| &all.frame.rgba[x * 4..x * 4 + 4];
        // SDR monitor 1: pixel (1, 0) is [1, 0, 1] in BGRA, so red = 1/255 (linear, tiny),
        // blue = monitor id 1 (also tiny); alpha 1. Its white would be 3.0 (240 nits).
        assert!(px(0)[0] < 0.01 && px(0)[3] == 1.0);
        // HDR monitor at 160 nits: its 2.0 (its SDR white) becomes 3.0, the canvas's.
        assert_eq!(&px(2)[..3], &[3.0, 3.0, 3.0]);
        // HDR monitor at 240 nits: untouched.
        assert_eq!(&px(4)[..3], &[2.0, 2.0, 2.0]);
        // A region of the HDR monitor keeps that monitor's levels.
        let region = c
            .grab_hdr(
                &CaptureTarget::Region {
                    monitor: MonitorId(2),
                    rect: PhysicalRect::new(1, 0, 1, 1),
                },
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(region.info, hdr(160.0, 600.0));
        assert_eq!(region.frame.rgba, vec![2.0; 4]);
        // An SDR-only target is not an HDR capture.
        assert!(
            c.grab_hdr(&CaptureTarget::Monitor(MonitorId(1)), false)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn one_capture_gives_the_sdr_and_hdr_pictures() {
        let mut monitors = vec![mon(1, 0, 0, 2, 1, 96), mon(2, 2, 0, 2, 1, 96)];
        monitors[1].hdr = Some(hdr(160.0, 600.0));
        let tonemap: crate::ToneMapFn = |rgba, _| [7, 7, 7, 255].repeat(rgba.len() / 4);
        let options = CaptureOptions {
            tonemap: Some(tonemap),
            ..CaptureOptions::default()
        };
        let mut apart = Capturer::new(FakeBackend::default(), monitors.clone());
        let sdr = apart.grab(&CaptureTarget::AllMonitors, options).unwrap();
        let hdr = apart.grab_hdr(&CaptureTarget::AllMonitors, false).unwrap();
        let mut once = Capturer::new(FakeBackend::default(), monitors);
        assert_eq!(once.grab_all_both(false, tonemap).unwrap(), (sdr, hdr));
        assert_eq!(
            once.backend.grabs,
            vec!["monitor 1 cursor=false", "hdr monitor 2 cursor=false"]
        );
    }

    #[test]
    fn a_window_on_an_hdr_monitor_is_hdr_and_tone_mapped_for_sdr() {
        let mut monitors = vec![mon(1, 0, 0, 4, 2, 96)];
        monitors[0].hdr = Some(hdr(200.0, 800.0));
        let backend = FakeBackend {
            window_monitor: Some(MonitorId(1)),
            ..FakeBackend::default()
        };
        let mut c = Capturer::new(backend, monitors);
        let window = CaptureTarget::Window(WindowId(7));
        let kept = c.grab_hdr(&window, true).unwrap().unwrap();
        assert_eq!(kept.info, hdr(200.0, 800.0));
        assert_eq!((kept.frame.width, kept.frame.height), (64, 48));
        let options = CaptureOptions {
            tonemap: Some(|rgba, info| {
                [(info.peak_nits / 10.0) as u8, 0, 0, 255].repeat(rgba.len() / 4)
            }),
            ..CaptureOptions::default()
        };
        let sdr = c.grab(&window, options).unwrap();
        assert_eq!(sdr.pixel(0, 0), [80, 0, 0, 255]);
        assert_eq!(
            c.backend.grabs,
            vec!["hdr window 7 cursor=true", "hdr window 7 cursor=false"]
        );
        // On an SDR monitor (or unknown), the plain capture.
        c.backend.window_monitor = None;
        assert!(c.grab_hdr(&window, false).unwrap().is_none());
        c.grab(&window, options).unwrap();
        assert_eq!(c.backend.grabs.last().unwrap(), "window 7 cursor=false");
    }
}
