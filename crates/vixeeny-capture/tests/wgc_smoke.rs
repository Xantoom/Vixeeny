// SPDX-License-Identifier: GPL-3.0-or-later
//! Real capture on Windows. CI runners may have no usable display, so an unavailable backend is reported and skipped;
//! when it works, the frame must match the monitor's physical size.

use std::time::Instant;

use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, WgcBackend};

#[test]
fn captures_small_regions_with_and_without_the_cursor() {
    vixeeny_platform::ensure_dpi_aware();
    let Ok(monitors) = vixeeny_platform::monitors() else {
        vixeeny_common::skip_test("no monitors");
        return;
    };
    let Ok(backend) = WgcBackend::new() else {
        vixeeny_common::skip_test("no capture device");
        return;
    };
    let mut capturer = Capturer::new(backend, monitors.clone());
    for monitor in monitors.iter().filter(|m| m.rect.width >= 337 && m.rect.height >= 223) {
        for show_cursor in [false, true] {
            let target = CaptureTarget::Region {
                monitor: monitor.id,
                rect: vixeeny_platform::PhysicalRect::new(17, 23, 320, 200),
            };
            let start = Instant::now();
            match capturer.grab(&target, CaptureOptions { show_cursor, ..Default::default() }) {
                Ok(frame) => {
                    assert_eq!((frame.width, frame.height), (320, 200));
                    assert!(frame.data.len() < monitor.rect.width as usize * monitor.rect.height as usize * 4);
                    eprintln!(
                        "region {} cursor={show_cursor}: {:?}, {} bytes read back",
                        monitor.name,
                        start.elapsed(),
                        frame.data.len()
                    );
                }
                Err(e) => vixeeny_common::skip_test(format!("capture unavailable: {e}")),
            }
        }
    }
}

#[test]
fn grabs_every_monitor_at_physical_size() {
    vixeeny_platform::ensure_dpi_aware();
    let monitors = match vixeeny_platform::monitors() {
        Ok(m) if !m.is_empty() => m,
        other => {
            vixeeny_common::skip_test(format!("no monitors ({other:?})"));
            return;
        }
    };
    for m in &monitors {
        eprintln!("monitor {} {:?} dpi {}", m.name, m.rect, m.dpi);
    }
    let backend = match WgcBackend::new() {
        Ok(b) => b,
        Err(e) => {
            vixeeny_common::skip_test(&e);
            return;
        }
    };
    let mut capturer = Capturer::new(backend, monitors.clone());
    for m in &monitors {
        let t = Instant::now();
        match capturer.grab(&CaptureTarget::Monitor(m.id), CaptureOptions::default()) {
            Ok(frame) => {
                eprintln!("grabbed {} in {:?}", m.name, t.elapsed());
                assert_eq!((frame.width, frame.height), (m.rect.width, m.rect.height));
            }
            Err(e) => vixeeny_common::skip_test(format!("capture failed: {e}")),
        }
    }
}

/// The replay films a game's window alone: a window stream delivers its frames, at its size.
#[test]
fn streams_a_window_at_its_size() {
    use std::time::Duration;
    use vixeeny_capture::{StreamTarget, VideoStream};

    vixeeny_platform::ensure_dpi_aware();
    let windows = vixeeny_platform::top_level_windows().unwrap_or_default();
    let Some(window) = windows.iter().find(|w| w.rect.width >= 200 && w.rect.height >= 200) else {
        vixeeny_common::skip_test("no window");
        return;
    };
    let stream = match VideoStream::start(&StreamTarget::Window(window.id), false, false) {
        Ok(s) => s,
        Err(e) => {
            vixeeny_common::skip_test(&e);
            return;
        }
    };
    match stream.recv(Duration::from_secs(2)) {
        Ok(Some(captured)) => {
            let (w, h) = (captured.frame.width, captured.frame.height);
            eprintln!("window {:?}: {w}x{h} (frame {:?})", window.title, window.rect);
            // The capture holds the window's frame, which may differ from its visible rectangle by its invisible
            // borders.
            assert!(w.abs_diff(window.rect.width) <= 32 && h.abs_diff(window.rect.height) <= 32);
        }
        Ok(None) => eprintln!("no frame within 2 s (nothing changed on screen)"),
        Err(e) => eprintln!("window capture failed (tolerated on CI): {e}"),
    }
}

/// An HDR screen read as scRGB, its windows too; an SDR-only setup has no HDR capture.
#[test]
fn hdr_screens_are_captured_as_scrgb() {
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors().unwrap_or_default();
    let backend = match WgcBackend::new() {
        Ok(b) => b,
        Err(e) => {
            vixeeny_common::skip_test(&e);
            return;
        }
    };
    let mut capturer = Capturer::new(backend, monitors.clone());
    let all = capturer.grab_hdr(&CaptureTarget::AllMonitors, false).unwrap();
    let Some(all) = all else {
        assert!(monitors.iter().all(|m| m.hdr.is_none()));
        eprintln!("no HDR screen");
        return;
    };
    eprintln!("HDR desktop {:?}", all.info);
    assert!(all.frame.rgba.iter().all(|v| v.is_finite()));
    for m in monitors.iter().filter(|m| m.hdr.is_some()) {
        let one = capturer.grab_hdr(&CaptureTarget::Monitor(m.id), false).unwrap().unwrap();
        assert_eq!((one.frame.width, one.frame.height), (m.rect.width, m.rect.height));
    }
    let on_hdr = vixeeny_platform::top_level_windows().unwrap_or_default().into_iter().find(|w| {
        w.rect.width >= 200
            && vixeeny_platform::monitor_at(&monitors, w.rect.x + 100, w.rect.y + 100).is_some_and(|m| m.hdr.is_some())
    });
    if let Some(window) = on_hdr {
        match capturer.grab_hdr(&CaptureTarget::Window(window.id), false) {
            Ok(Some(c)) => eprintln!("HDR window {:?}: {}x{}", window.title, c.frame.width, c.frame.height),
            Ok(None) => eprintln!("window {:?} is mostly on an SDR screen", window.title),
            Err(e) => eprintln!("window capture failed (tolerated): {e}"),
        }
    }
}

/// A recording's gate sees every frame before it is read: the stream hands over those it wants, and only those.
#[test]
fn a_gated_stream_hands_over_only_the_wanted_frames() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use vixeeny_capture::{Gate, Pacing, StreamTarget, VideoStream};

    /// Wants the first frame only; counts what it is asked and given.
    #[derive(Default)]
    struct First {
        asked: AtomicU32,
        taken: AtomicU32,
    }
    impl Gate for First {
        fn wanted(&self, _: i64) -> bool {
            self.asked.fetch_add(1, Ordering::Relaxed);
            self.taken.load(Ordering::Relaxed) == 0
        }
        fn taken(&self, _: i64) {
            self.taken.fetch_add(1, Ordering::Relaxed);
        }
    }

    vixeeny_platform::ensure_dpi_aware();
    let Some(monitor) = vixeeny_platform::monitors().ok().and_then(|m| m.into_iter().next()) else {
        vixeeny_common::skip_test("no monitor");
        return;
    };
    let gate = Arc::new(First::default());
    let pacing = Pacing { fps: (60, 1), gate: Arc::clone(&gate) as Arc<dyn Gate> };
    let stream = match VideoStream::start_with(&StreamTarget::Monitor(monitor), false, false, None, Some(pacing)) {
        Ok(s) => s,
        Err(e) => {
            vixeeny_common::skip_test(&e);
            return;
        }
    };
    let mut received = 0;
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_millis(500) {
        if let Ok(Some(_)) = stream.recv(Duration::from_millis(50)) {
            received += 1;
        }
    }
    drop(stream);
    eprintln!(
        "asked {}, taken {}, received {received}",
        gate.asked.load(Ordering::Relaxed),
        gate.taken.load(Ordering::Relaxed)
    );
    assert!(gate.asked.load(Ordering::Relaxed) >= 1, "the first frame is asked about");
    assert_eq!(gate.taken.load(Ordering::Relaxed), 1);
    assert_eq!(received, 1, "the frames the gate did not want were not read");
}
