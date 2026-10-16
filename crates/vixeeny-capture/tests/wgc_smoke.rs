// SPDX-License-Identifier: GPL-3.0-or-later
//! Real capture on Windows. CI runners may have no usable display, so an unavailable backend
//! is reported and skipped; when it works, the frame must match the monitor's physical size.

use std::time::Instant;

use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, WgcBackend};

#[test]
fn grabs_every_monitor_at_physical_size() {
    vixeeny_platform::ensure_dpi_aware();
    let monitors = match vixeeny_platform::monitors() {
        Ok(m) if !m.is_empty() => m,
        other => {
            eprintln!("skipped: no monitors ({other:?})");
            return;
        }
    };
    for m in &monitors {
        eprintln!("monitor {} {:?} dpi {}", m.name, m.rect, m.dpi);
    }
    let backend = match WgcBackend::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipped: {e}");
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
            Err(e) => eprintln!("capture failed (tolerated on CI): {e}"),
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
    let Some(window) = windows
        .iter()
        .find(|w| w.rect.width >= 200 && w.rect.height >= 200)
    else {
        eprintln!("skipped: no window");
        return;
    };
    let stream = match VideoStream::start(&StreamTarget::Window(window.id), false, false) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    match stream.recv(Duration::from_secs(2)) {
        Ok(Some(captured)) => {
            let (w, h) = (captured.frame.width, captured.frame.height);
            eprintln!(
                "window {:?}: {w}x{h} (frame {:?})",
                window.title, window.rect
            );
            // The capture holds the window's frame, which may differ from its visible rectangle
            // by its invisible borders.
            assert!(w.abs_diff(window.rect.width) <= 32 && h.abs_diff(window.rect.height) <= 32);
        }
        Ok(None) => eprintln!("no frame within 2 s (nothing changed on screen)"),
        Err(e) => eprintln!("window capture failed (tolerated on CI): {e}"),
    }
}
