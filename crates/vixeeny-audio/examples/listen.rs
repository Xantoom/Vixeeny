// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS check of the ScreenCaptureKit audio source: listens to the system sound for five seconds
//! (play something) and prints what arrived. `cargo run -p vixeeny-audio --example listen`.
//! Pass `app:<name>` or `mic` to listen to one application or the microphone instead.

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::{Arc, Mutex};
    use vixeeny_audio::{
        AudioChunk, AudioSink, AudioSource, SckAudioSource, SourceEvent, SourceKind,
    };

    #[derive(Default)]
    struct Stats {
        chunks: usize,
        frames: usize,
        peak: f32,
        first_ns: Option<i64>,
    }
    struct Collect(Arc<Mutex<Stats>>);
    impl AudioSink for Collect {
        fn on_audio(&mut self, chunk: AudioChunk) {
            if let Ok(mut s) = self.0.lock() {
                s.chunks += 1;
                s.frames += chunk.frames();
                s.first_ns.get_or_insert(chunk.time_ns);
                s.peak = chunk.samples.iter().fold(s.peak, |p, v| p.max(v.abs()));
            }
        }
        fn on_event(&mut self, event: SourceEvent) {
            println!("event: {event:?}");
        }
    }

    let kind = match std::env::args().nth(1).as_deref() {
        Some("mic") => SourceKind::Microphone(None),
        Some(other) if other.starts_with("app:") => {
            SourceKind::Application(other.trim_start_matches("app:").to_owned())
        }
        _ => SourceKind::System,
    };
    let stats = Arc::new(Mutex::new(Stats::default()));
    let mut source = SckAudioSource::new(kind);
    source.start(Box::new(Collect(stats.clone())))?;
    std::thread::sleep(std::time::Duration::from_secs(5));
    source.stop()?;
    if let Ok(s) = stats.lock() {
        println!(
            "{} chunks, {} frames ({:.2} s), peak {:.3}, first at {:?} ns (now {} ns)",
            s.chunks,
            s.frames,
            s.frames as f64 / 48_000.0,
            s.peak,
            s.first_ns,
            vixeeny_platform::monotonic_ns()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS only");
}
