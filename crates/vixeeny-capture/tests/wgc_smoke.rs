// SPDX-License-Identifier: GPL-3.0-or-later
//! Real capture on Windows. CI runners may have no usable display, so an unavailable backend
//! is reported and skipped; when it works, the frame must match the monitor's physical size.
#![cfg(windows)]

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
