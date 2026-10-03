// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS check of the whole chain: ScreenCaptureKit video + system sound → recorder → MP4.
//!
//! `cargo run -p vixeeny-encode --features ffmpeg-next --example record_mac -- [seconds] [encoder]`
//! records the first display (default 5 s, encoder `videotoolbox_h264`; `libx264` for software)
//! to `record-mac.mp4` in the current folder. Play some sound meanwhile and check that the
//! picture moves, the sound is there and both stay in sync (clap or click on camera-less cues).
#![cfg(target_os = "macos")]
#![allow(clippy::print_stdout)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use vixeeny_audio::{
    AudioChunk, AudioSink, AudioSource, Mixer, SckAudioSource, SourceEvent, SourceKind,
};
use vixeeny_capture::SckVideoStream;
use vixeeny_encode::audio::{AudioCodec, AudioTrackConfig};
use vixeeny_encode::clock::Fps;
use vixeeny_encode::recorder::{
    FrameFormat, OutputContainer, RecordConfig, Recorder, Split, VideoFrame,
};
use vixeeny_encode::registry::{Chroma, PresetName, Registry};

struct ToChannel(std::sync::mpsc::Sender<AudioChunk>);

impl AudioSink for ToChannel {
    fn on_audio(&mut self, chunk: AudioChunk) {
        let _ = self.0.send(chunk);
    }

    fn on_event(&mut self, event: SourceEvent) {
        println!("audio event: {event:?}");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let encoder_id = args.next().unwrap_or_else(|| "videotoolbox_h264".into());

    let monitors = vixeeny_platform::monitors()?;
    let monitor = monitors.first().ok_or("no display")?.clone();
    let registry = Registry::builtin()?;
    let encoder = registry
        .get(&encoder_id)
        .ok_or("unknown encoder id")?
        .clone();
    let options = encoder
        .presets
        .get(PresetName::Balanced)
        .iter()
        .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
        .collect();
    let size = (monitor.rect.width & !1, monitor.rect.height & !1);
    let config = RecordConfig {
        encoder,
        options,
        container: OutputContainer::Mp4Hybrid,
        output_size: size,
        fps: Fps::whole(30),
        depth: 8,
        chroma: Chroma::C420,
        hdr: false,
        split: Split::Off,
        keyframe_seconds: 2.0,
        queue: 8,
        vfr: false,
        audio: vec![AudioTrackConfig {
            title: "System".into(),
            codec: AudioCodec::Aac,
            bitrate_kbps: 160,
            vbr: true,
            channels: 2,
        }],
        gpu: None,
        replay_seconds: None,
        replay_storage: vixeeny_encode::replay::Storage::Ram,
        files: true,
    };
    let path = PathBuf::from("record-mac.mp4");
    let target = path.clone();
    let recorder = Recorder::start(config, Box::new(move |_| target.clone()))?;

    let origin = vixeeny_platform::monotonic_ns();
    let (audio_tx, audio_rx) = std::sync::mpsc::channel();
    let mut audio = SckAudioSource::new(SourceKind::System);
    if let Err(e) = audio.start(Box::new(ToChannel(audio_tx))) {
        println!("no system sound: {e}");
    }
    let stream = SckVideoStream::start_monitor(&monitor, 30, true)?;
    let mut mixer = Mixer::new(&[1.0], 0);

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        if let Some(f) = stream.recv(Duration::from_millis(50))? {
            let frame = VideoFrame {
                width: f.frame.width,
                height: f.frame.height,
                stride: f.frame.stride,
                format: FrameFormat::Bgra8,
                data: f.frame.data,
            };
            recorder.push_frame((f.time_ns - origin).max(0), frame);
        }
        let now = vixeeny_platform::monotonic_ns() - origin;
        while let Ok(mut chunk) = audio_rx.try_recv() {
            chunk.time_ns -= origin;
            mixer.push(0, &chunk);
        }
        for block in mixer.drain(now) {
            recorder.push_audio(0, block.samples);
        }
        recorder.tick(now);
    }
    let end = vixeeny_platform::monotonic_ns() - origin;
    drop(stream);
    let _ = audio.stop();
    for block in mixer.finish(end) {
        recorder.push_audio(0, block.samples);
    }
    let summary = recorder.stop(end)?;
    println!("{summary:?}");
    println!(
        "wrote {} — open it and check picture, sound and sync",
        path.display()
    );
    Ok(())
}
