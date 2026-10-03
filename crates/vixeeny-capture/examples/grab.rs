// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS check of the ScreenCaptureKit backend: grabs every monitor and writes `grab-<n>.ppm`
//! (open them with Preview). `cargo run -p vixeeny-capture --example grab`.

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, SckBackend};

    let monitors = vixeeny_platform::monitors()?;
    for m in &monitors {
        println!("{m:?}");
    }
    println!("cursor: {:?}", vixeeny_platform::cursor_position());
    let backend = SckBackend::new()?;
    let mut capturer = Capturer::new(backend, monitors.clone());
    for (n, m) in monitors.iter().enumerate() {
        let started = std::time::Instant::now();
        let frame = capturer.grab(&CaptureTarget::Monitor(m.id), CaptureOptions::default())?;
        println!(
            "{}x{} in {:?}",
            frame.width,
            frame.height,
            started.elapsed()
        );
        let mut out = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
        for y in 0..frame.height {
            for px in frame.row(y).as_chunks::<4>().0 {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        std::fs::write(format!("grab-{n}.ppm"), out)?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS only");
}
