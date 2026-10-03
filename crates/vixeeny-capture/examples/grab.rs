// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS / Linux (X11) check of the screen backends: grabs every monitor and writes `grab-<n>.ppm`
//! (open them with Preview). `cargo run -p vixeeny-capture --example grab`.

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    use vixeeny_capture::X11VideoStream as Stream;
    use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer};
    #[cfg(target_os = "macos")]
    use vixeeny_capture::{SckBackend as Backend, SckVideoStream as Stream};

    let monitors = vixeeny_platform::monitors()?;
    for m in &monitors {
        println!("{m:?}");
    }
    println!("cursor: {:?}", vixeeny_platform::cursor_position());
    #[cfg(target_os = "macos")]
    let backend = Backend::new()?;
    #[cfg(target_os = "linux")]
    let backend = vixeeny_capture::LinuxBackend::new(&monitors)?;
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
    let windows = vixeeny_platform::top_level_windows()?;
    for w in windows.iter().take(8) {
        println!(
            "window {:?}: {:?} {:?} {:?}",
            w.id, w.title, w.rect, w.exe_path
        );
    }
    if let Some(w) = windows.first() {
        let frame = capturer.grab(&CaptureTarget::Window(w.id), CaptureOptions::default())?;
        println!("front window: {}x{}", frame.width, frame.height);
    }
    // A two-second video stream on the first monitor: frames per second and clock sanity.
    if let Some(m) = monitors.first() {
        let stream = Stream::start_monitor(m, 30, false)?;
        let started = std::time::Instant::now();
        let (mut count, mut last_ns) = (0u32, 0i64);
        while started.elapsed() < std::time::Duration::from_secs(2) {
            if let Some(f) = stream.recv(std::time::Duration::from_millis(200))? {
                count += 1;
                last_ns = f.time_ns;
            }
        }
        println!(
            "stream: {count} frames in 2 s; last frame {} ms before now",
            (vixeeny_platform::monotonic_ns() - last_ns) / 1_000_000
        );
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("macOS and Linux only");
}
