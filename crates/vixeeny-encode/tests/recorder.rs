// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording pipeline end to end with synthetic frames and a simulated clock:
//! CA-REC-2 (crash-safe hybrid MP4), CA-REC-3 (pause), CA-REC-6 (every software encoder × every
//! allowed container gives a readable file), constant frame rate, scaling, splitting.
//! Needs the native build and `--features ffmpeg-next`.
#![cfg(feature = "ffmpeg-next")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vixeeny_encode::clock::Fps;
use vixeeny_encode::ffmpeg::{codec, format, media};
use vixeeny_encode::recorder::{
    FrameFormat, OutputContainer, RecordConfig, Recorder, Split, VideoFrame,
};
use vixeeny_encode::registry::{Chroma, PresetName, Registry};

const MS: i64 = 1_000_000;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vixeeny-rec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A moving gradient, so that every frame differs.
fn frame(w: u32, h: u32, i: usize) -> VideoFrame {
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            bgra.extend_from_slice(&[
                ((x as usize + i * 4) % 256) as u8,
                ((y as usize + i * 2) % 256) as u8,
                ((x + y) as usize / 2 % 256) as u8,
                255,
            ]);
        }
    }
    VideoFrame {
        width: w,
        height: h,
        stride: (w * 4) as usize,
        format: FrameFormat::Bgra8,
        data: bgra,
    }
}

fn config(registry: &Registry, id: &str, container: OutputContainer, fps: u32) -> RecordConfig {
    let encoder = registry.get(id).unwrap().clone();
    let mut options: Vec<(String, String)> = encoder
        .presets
        .get(PresetName::Performance)
        .iter()
        .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
        .collect();
    // Faster than the user presets: this is a test.
    match id {
        "libsvtav1" => options.retain(|(k, _)| k != "preset"),
        "libx265" => options.push(("x265-params".into(), "log-level=none".into())),
        _ => {}
    }
    if id == "libsvtav1" {
        options.push(("preset".into(), "12".into()));
    }
    RecordConfig {
        encoder,
        options,
        container,
        output_size: (320, 180),
        fps: Fps::whole(fps),
        depth: 8,
        chroma: Chroma::C420,
        hdr: false,
        split: Split::Off,
        keyframe_seconds: 1.0,
        queue: 1_000,
        vfr: false,
        audio: Vec::new(),
        gpu: None,
        replay_seconds: None,
        replay_storage: vixeeny_encode::replay::Storage::Ram,
        files: true,
    }
}

fn namer(dir: &Path, ext: &str) -> Box<dyn FnMut(u32) -> PathBuf + Send> {
    let dir = dir.to_path_buf();
    let ext = ext.to_owned();
    Box::new(move |part| dir.join(format!("rec-{part}.{ext}")))
}

/// What a demuxer sees in the video stream.
#[derive(Debug)]
struct Seen {
    codec: codec::Id,
    pts_ms: Vec<i64>,
    keyframes: usize,
    size: (u32, u32),
}

fn demux(path: &Path) -> Seen {
    let mut input = format::input(path).unwrap();
    let stream = input.streams().best(media::Type::Video).unwrap();
    let index = stream.index();
    let tb = stream.time_base();
    let params = codec::context::Context::from_parameters(stream.parameters()).unwrap();
    let video = params.decoder().video().unwrap();
    let (codec, size) = (video.id(), (video.width(), video.height()));
    let mut pts_ms = Vec::new();
    let mut keyframes = 0;
    for (s, packet) in input.packets() {
        if s.index() != index {
            continue;
        }
        let Some(pts) = packet.pts() else { continue };
        pts_ms.push(pts * 1000 * i64::from(tb.0) / i64::from(tb.1));
        keyframes += usize::from(packet.is_key());
    }
    pts_ms.sort_unstable();
    Seen {
        codec,
        pts_ms,
        keyframes,
        size,
    }
}

/// Records `n` frames at `fps` through a fresh recorder.
fn record(cfg: RecordConfig, dir: &Path, n: usize) -> vixeeny_encode::recorder::Summary {
    let fps = i64::from(cfg.fps.num);
    let ext = cfg.container.extension();
    let rec = Recorder::start(cfg, namer(dir, ext)).unwrap();
    for i in 0..n {
        assert!(rec.push_frame(i as i64 * 1_000 * MS / fps, frame(320, 180, i)));
    }
    rec.stop(n as i64 * 1_000 * MS / fps).unwrap()
}

#[test]
fn every_software_encoder_in_every_allowed_container_is_readable() {
    // CA-REC-6 for what runs everywhere.
    let registry = Registry::builtin().unwrap();
    let dir = scratch("matrix");
    for id in ["libx264", "libx265", "libsvtav1", "libvpx_vp9"] {
        let family = registry.get(id).unwrap().family;
        for (container, allowed) in [
            (OutputContainer::Mkv, true),
            (OutputContainer::Mp4Hybrid, true),
            (OutputContainer::Mp4Fragmented, true),
            (
                OutputContainer::WebM,
                matches!(family.name(), "av1" | "vp9"),
            ),
        ] {
            if !allowed {
                continue;
            }
            let sub = dir.join(format!("{id}-{container:?}"));
            let summary = record(config(&registry, id, container, 30), &sub, 30);
            let seen = demux(&summary.files[0]);
            assert_eq!(seen.pts_ms.len(), 30, "{id} {container:?}");
            assert_eq!(seen.size, (320, 180));
            let want = match family.name() {
                "h264" => codec::Id::H264,
                "hevc" => codec::Id::HEVC,
                "av1" => codec::Id::AV1,
                _ => codec::Id::VP9,
            };
            assert_eq!(seen.codec, want, "{id} {container:?}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn output_is_constant_frame_rate_whatever_the_input_timing() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("cfr");
    let rec = Recorder::start(
        config(&registry, "libx264", OutputContainer::Mkv, 25),
        namer(&dir, "mkv"),
    )
    .unwrap();
    // 2 s of wall clock with jitter, a burst, and a 300 ms stall.
    let times: Vec<i64> = [
        0, 41, 83, 118, 160, 162, 200, 241, 600, 640, 679, 720, 760, 801, 1_000, 1_500, 1_960,
    ]
    .iter()
    .map(|ms| ms * MS)
    .collect();
    for (i, t) in times.iter().enumerate() {
        rec.push_frame(*t, frame(320, 180, i));
    }
    let summary = rec.stop(2_000 * MS).unwrap();
    assert_eq!(summary.frames, 50, "2 s at 25 fps");
    let seen = demux(&summary.files[0]);
    assert_eq!(seen.pts_ms.len(), 50);
    for (i, pts) in seen.pts_ms.iter().enumerate() {
        assert_eq!(*pts, i as i64 * 40, "frame {i}");
    }
    assert!(
        summary.repeated > 0 && summary.dropped_by_clock > 0,
        "{summary:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pause_leaves_no_hole_and_the_duration_excludes_it() {
    // CA-REC-3.
    let registry = Registry::builtin().unwrap();
    let dir = scratch("pause");
    let rec = Recorder::start(
        config(&registry, "libx264", OutputContainer::Mkv, 30),
        namer(&dir, "mkv"),
    )
    .unwrap();
    for i in 0..30 {
        rec.push_frame(i * 1_000 * MS / 30, frame(320, 180, i as usize)); // 0 – 1 s
    }
    rec.pause(1_000 * MS);
    for i in 0..30 {
        rec.push_frame(1_000 * MS + i * 33 * MS, frame(320, 180, 0)); // ignored: paused
    }
    rec.resume(5_000 * MS); // 4 s pause
    for i in 0..30 {
        rec.push_frame(
            5_000 * MS + i * 1_000 * MS / 30,
            frame(320, 180, i as usize),
        );
    }
    let summary = rec.stop(6_000 * MS).unwrap();
    let seen = demux(&summary.files[0]);
    // 2 s of recording: 60 frames (±1), evenly spaced, no gap.
    assert!(
        (59..=61).contains(&seen.pts_ms.len()),
        "{}",
        seen.pts_ms.len()
    );
    for pair in seen.pts_ms.windows(2) {
        let step = pair[1] - pair[0];
        assert!((32..=34).contains(&step), "gap of {step} ms");
    }
    let duration = seen.pts_ms.last().unwrap() - seen.pts_ms[0];
    assert!((1_933..=2_000).contains(&duration), "{duration}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_static_screen_keeps_its_frame_rate_through_ticks() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("tick");
    let rec = Recorder::start(
        config(&registry, "libx264", OutputContainer::Mkv, 20),
        namer(&dir, "mkv"),
    )
    .unwrap();
    rec.push_frame(0, frame(320, 180, 0)); // nothing changes afterwards
    for ms in (100..=1_000).step_by(100) {
        rec.tick(ms * MS);
    }
    let summary = rec.stop(1_000 * MS).unwrap();
    assert_eq!(summary.frames, 20);
    assert_eq!(demux(&summary.files[0]).pts_ms.len(), 20);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn frames_are_scaled_to_the_output_size() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("scale");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.output_size = (160, 90);
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    for i in 0..10 {
        rec.push_frame(i * 33 * MS, frame(640, 360, i as usize)); // a 4× larger source
    }
    let summary = rec.stop(330 * MS).unwrap();
    assert_eq!(demux(&summary.files[0]).size, (160, 90));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ten_bit_and_444_formats_record() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("formats");
    for (id, depth, chroma) in [
        ("libx264", 10, Chroma::C420),
        ("libx264", 8, Chroma::C444),
        ("libx265", 10, Chroma::C420),
    ] {
        let mut cfg = config(&registry, id, OutputContainer::Mkv, 30);
        cfg.depth = depth;
        cfg.chroma = chroma;
        let summary = record(
            cfg,
            &dir.join(format!("{id}-{depth}-{}", chroma.name())),
            10,
        );
        assert_eq!(
            demux(&summary.files[0]).pts_ms.len(),
            10,
            "{id} {depth} {chroma:?}"
        );
    }
    // An encoder that cannot do it is refused up front.
    let mut cfg = config(&registry, "libsvtav1", OutputContainer::Mkv, 30);
    cfg.chroma = Chroma::C444;
    assert!(Recorder::start(cfg, namer(&dir, "mkv")).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn hdr_streams_are_tagged_bt2020_pq() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("hdr");
    let mut cfg = config(&registry, "libx265", OutputContainer::Mkv, 30);
    cfg.depth = 10;
    cfg.hdr = true;
    let summary = record(cfg, &dir, 10);
    let input = format::input(&summary.files[0]).unwrap();
    let stream = input.streams().best(media::Type::Video).unwrap();
    let ctx = codec::context::Context::from_parameters(stream.parameters()).unwrap();
    let video = ctx.decoder().video().unwrap();
    assert_eq!(
        video.color_primaries(),
        vixeeny_encode::ffmpeg::color::Primaries::BT2020
    );
    assert_eq!(
        video.color_transfer_characteristic(),
        vixeeny_encode::ffmpeg::color::TransferCharacteristic::SMPTE2084
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn splitting_by_duration_cuts_on_key_frames_and_loses_nothing() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("split");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.split = Split::Duration(Duration::from_secs(2));
    cfg.keyframe_seconds = 1.0;
    let summary = record(cfg, &dir, 30 * 7);
    assert!(summary.files.len() >= 3, "{:?}", summary.files);
    let mut total = 0;
    for file in &summary.files {
        let seen = demux(file);
        assert_eq!(seen.pts_ms[0], 0, "each part starts at zero");
        assert!(seen.keyframes >= 1);
        total += seen.pts_ms.len();
    }
    assert_eq!(total, 30 * 7, "no frame lost between parts");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn splitting_by_size_makes_several_playable_files() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("splitsize");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mp4Fragmented, 30);
    cfg.split = Split::Bytes(20_000);
    cfg.keyframe_seconds = 0.5;
    let summary = record(cfg, &dir, 30 * 6);
    assert!(summary.files.len() >= 2, "{}", summary.files.len());
    let total: usize = summary.files.iter().map(|f| demux(f).pts_ms.len()).sum();
    assert_eq!(total, 30 * 6);
    let _ = std::fs::remove_dir_all(dir);
}

/// Top-level boxes of an MP4 file, in order.
fn boxes(path: &Path) -> Vec<String> {
    let data = std::fs::read(path).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        out.push(String::from_utf8_lossy(&data[at + 4..at + 8]).into_owned());
        let size = if size == 0 { data.len() - at } else { size };
        if size < 8 {
            break;
        }
        at += size;
    }
    out
}

#[test]
fn hybrid_mp4_ends_classic_and_fragmented_mp4_stays_fragmented() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("hybrid");
    let hybrid = record(
        config(&registry, "libx264", OutputContainer::Mp4Hybrid, 30),
        &dir.join("h"),
        60,
    );
    let b = boxes(&hybrid.files[0]);
    assert!(!b.contains(&"moof".to_string()), "{b:?}");
    let (moov, mdat) = (
        b.iter().position(|x| x == "moov"),
        b.iter().position(|x| x == "mdat"),
    );
    assert!(
        moov.unwrap() < mdat.unwrap(),
        "faststart: moov before mdat, {b:?}"
    );
    assert_eq!(demux(&hybrid.files[0]).pts_ms.len(), 60);

    let fragmented = record(
        config(&registry, "libx264", OutputContainer::Mp4Fragmented, 30),
        &dir.join("f"),
        60,
    );
    let b = boxes(&fragmented.files[0]);
    assert!(b.contains(&"moof".to_string()), "{b:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_hybrid_recording_cut_short_is_readable_up_to_the_last_fragment() {
    // CA-REC-2: the file of a recording still in progress (what a crash leaves behind) opens and
    // holds every fragment written so far. The copy is taken while the recorder is running.
    let registry = Registry::builtin().unwrap();
    let dir = scratch("crash");
    let cfg = config(&registry, "libx264", OutputContainer::Mp4Hybrid, 30);
    let rec = Recorder::start(cfg, namer(&dir, "mp4")).unwrap();
    let live = dir.join("rec-0.mp4");
    let copy = dir.join("crashed.mp4");
    let snapshot = Arc::new(Mutex::new(0usize));
    for i in 0..30 * 5 {
        rec.push_frame(i as i64 * 1_000 * MS / 30, frame(320, 180, i));
        if i == 30 * 4 {
            // Let the worker catch up, then copy the half-written file, as a kill would leave it.
            while rec
                .stats()
                .encoded
                .load(std::sync::atomic::Ordering::Relaxed)
                < 30 * 4
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(200));
            std::fs::copy(&live, &copy).unwrap();
            *snapshot.lock().unwrap() = 1;
        }
    }
    let _ = rec.stop(5_000 * MS).unwrap();
    assert_eq!(*snapshot.lock().unwrap(), 1);
    eprintln!(
        "copy: {} bytes, boxes {:?}",
        std::fs::metadata(&copy).unwrap().len(),
        boxes(&copy)
    );
    let seen = demux(&copy);
    // Key frames every second: at least the first three seconds are complete fragments.
    assert!(
        seen.pts_ms.len() >= 60,
        "only {} frames survived",
        seen.pts_ms.len()
    );
    assert!(seen.pts_ms.len() <= 30 * 5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_slow_encoder_drops_input_frames_instead_of_blocking() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("queue");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.queue = 1;
    cfg.output_size = (1920, 1080);
    // A slow preset on a big frame, fed as fast as possible.
    cfg.options = vec![
        ("preset".into(), "veryslow".into()),
        ("crf".into(), "18".into()),
    ];
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    let started = std::time::Instant::now();
    let mut refused = 0;
    // Built once: a fast producer is what overflows the queue, even on a quick machine.
    let big = frame(1920, 1080, 0);
    for i in 0..200 {
        if !rec.push_frame(i * 33 * MS, big.clone()) {
            refused += 1;
        }
    }
    assert!(refused > 0, "the queue must overflow");
    assert_eq!(
        rec.stats()
            .queue_dropped
            .load(std::sync::atomic::Ordering::Relaxed),
        refused
    );
    let summary = rec.stop(200 * 33 * MS).unwrap();
    assert_eq!(summary.dropped_by_queue, refused);
    let _ = started;
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn variable_frame_rate_keeps_every_frame_at_its_own_time() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("vfr");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.vfr = true;
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    // Irregular times, a long still period, then a pause that must be cut out.
    let times_ms = [0i64, 10, 40, 41, 500, 2_000, 2_033];
    for (i, t) in times_ms.iter().enumerate() {
        assert!(rec.push_frame(t * MS, frame(320, 180, i)));
    }
    rec.tick(2_500 * MS); // ignored in VFR
    rec.pause(2_100 * MS);
    rec.push_frame(3_000 * MS, frame(320, 180, 0)); // paused: ignored
    rec.resume(5_100 * MS); // 3 s pause
    rec.push_frame(5_200 * MS, frame(320, 180, 7));
    let summary = rec.stop(5_300 * MS).unwrap();
    assert_eq!(summary.frames, 8, "no frame repeated or dropped");
    assert_eq!(summary.repeated, 0);
    let seen = demux(&summary.files[0]);
    // Same offsets, the 3 s pause removed (5 200 − 3 000 = 2 200).
    let expected = [0, 10, 40, 41, 500, 2_000, 2_033, 2_200];
    assert_eq!(seen.pts_ms.len(), expected.len(), "{:?}", seen.pts_ms);
    for (got, want) in seen.pts_ms.iter().zip(expected) {
        assert!((got - want).abs() <= 1, "{:?}", seen.pts_ms);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn variable_frame_rate_is_refused_in_mp4() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("vfr-mp4");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mp4Hybrid, 30);
    cfg.vfr = true;
    assert!(Recorder::start(cfg, namer(&dir, "mp4")).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

/// A uniform scRGB frame (all channels `half`, as raw half-float bits).
fn hdr_frame(w: u32, h: u32, half: u16) -> VideoFrame {
    let px: Vec<u8> = [half, half, half, 0x3C00]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    VideoFrame {
        width: w,
        height: h,
        stride: (w * 8) as usize,
        format: FrameFormat::ScRgbHalf,
        data: px.repeat((w * h) as usize),
    }
}

#[test]
fn scrgb_frames_come_out_as_bt2020_pq_10_bit() {
    use vixeeny_encode::ffmpeg::frame;
    let registry = Registry::builtin().unwrap();
    let dir = scratch("scrgb");
    let mut cfg = config(&registry, "libx265", OutputContainer::Mkv, 30);
    cfg.depth = 10;
    cfg.hdr = true;
    let fps = 30;
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    // 1.25 in scRGB = 100 nits = PQ 0.508.
    for i in 0..10 {
        rec.push_frame(i * 1_000 * MS / fps, hdr_frame(320, 180, 0x3D00));
    }
    let summary = rec.stop(10 * 1_000 * MS / fps).unwrap();

    let mut input = format::input(&summary.files[0]).unwrap();
    let index = input.streams().best(media::Type::Video).unwrap().index();
    let params = input.stream(index).unwrap().parameters();
    let mut decoder = codec::context::Context::from_parameters(params)
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let mut decoded = frame::Video::empty();
    'outer: for (s, packet) in input.packets() {
        if s.index() != index {
            continue;
        }
        decoder.send_packet(&packet).unwrap();
        if decoder.receive_frame(&mut decoded).is_ok() {
            break 'outer;
        }
    }
    assert_eq!(
        decoded.format(),
        vixeeny_encode::ffmpeg::format::Pixel::YUV420P10LE
    );
    // Luma of the centre pixel, 10-bit limited range: 64 + 876 × 0.508 ≈ 509.
    let stride = decoded.stride(0);
    let at = 90 * stride + 160 * 2;
    let y = i32::from(u16::from_le_bytes([
        decoded.data(0)[at],
        decoded.data(0)[at + 1],
    ]));
    assert!((y - 509).abs() <= 6, "luma {y}");
    // Neutral grey: chroma at mid-scale (512).
    let cb = i32::from(u16::from_le_bytes([decoded.data(1)[0], decoded.data(1)[1]]));
    assert!((cb - 512).abs() <= 4, "cb {cb}");
    let _ = std::fs::remove_dir_all(dir);
}

// ---- audio ----

use vixeeny_encode::audio::{AudioCodec, AudioTrackConfig};

fn audio_track(title: &str, codec: AudioCodec) -> AudioTrackConfig {
    AudioTrackConfig {
        title: title.into(),
        codec,
        bitrate_kbps: 128,
        vbr: true,
        channels: 2,
    }
}

/// `seconds` of a 440 Hz tone for the recorder: blocks of 20 ms, like the mixer produces.
fn tone_blocks(seconds: usize, mut first_frame: usize) -> Vec<Vec<f32>> {
    (0..seconds * 50)
        .map(|_| {
            let block: Vec<f32> = (0..960)
                .flat_map(|i| {
                    let n = (first_frame + i) as f32;
                    let v = (n / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                    [v, v]
                })
                .collect();
            first_frame += 960;
            block
        })
        .collect()
}

struct AudioSeen {
    /// (codec, first pts ms, last end ms, title) per audio stream.
    streams: Vec<(codec::Id, i64, i64, Option<String>)>,
}

fn demux_audio(path: &Path) -> AudioSeen {
    let mut input = format::input(path).unwrap();
    let audio: Vec<usize> = input
        .streams()
        .filter(|s| s.parameters().medium() == media::Type::Audio)
        .map(|s| s.index())
        .collect();
    let mut firsts = vec![i64::MAX; audio.len()];
    let mut lasts = vec![i64::MIN; audio.len()];
    let tbs: Vec<_> = audio
        .iter()
        .map(|i| input.stream(*i).unwrap().time_base())
        .collect();
    for (s, packet) in input.packets() {
        if let Some(k) = audio.iter().position(|i| *i == s.index()) {
            let Some(pts) = packet.pts() else { continue };
            let ms = |v: i64| v * 1000 * i64::from(tbs[k].0) / i64::from(tbs[k].1);
            firsts[k] = firsts[k].min(ms(pts));
            lasts[k] = lasts[k].max(ms(pts + packet.duration()));
        }
    }
    AudioSeen {
        streams: audio
            .iter()
            .enumerate()
            .map(|(k, i)| {
                let st = input.stream(*i).unwrap();
                let meta = st.metadata();
                (
                    st.parameters().id(),
                    firsts[k],
                    lasts[k],
                    meta.get("title")
                        .or_else(|| meta.get("handler_name"))
                        .map(str::to_owned),
                )
            })
            .collect(),
    }
}

/// Records `seconds` of video at 30 fps plus the given audio tracks (the same tone in each).
fn record_av(
    cfg: RecordConfig,
    dir: &Path,
    seconds: usize,
    video_start_ms: i64,
) -> vixeeny_encode::recorder::Summary {
    record_av_channels(cfg, dir, seconds, video_start_ms, 2)
}

/// As [`record_av`], the tone on the front pair of tracks that have `channels` channels.
fn record_av_channels(
    cfg: RecordConfig,
    dir: &Path,
    seconds: usize,
    video_start_ms: i64,
    channels: usize,
) -> vixeeny_encode::recorder::Summary {
    let ext = cfg.container.extension();
    let tracks = cfg.audio.len();
    let rec = Recorder::start(cfg, namer(dir, ext)).unwrap();
    let blocks = tone_blocks(seconds, 0);
    for (k, block) in blocks.into_iter().enumerate() {
        let block: Vec<f32> = block
            .chunks(2)
            .flat_map(|f| {
                let mut frame = vec![0.0; channels];
                frame[..2].copy_from_slice(f);
                frame
            })
            .collect();
        let t = k as i64 * 20 * MS;
        // 0.6 video frames per audio block: push the frames whose time has come.
        for i in 0..tracks {
            rec.push_audio(i, block.clone());
        }
        let video_t = t - video_start_ms * MS;
        if video_t >= 0 && k % 2 == 0 {
            let n = (video_t / (1_000 * MS / 30)) as usize;
            rec.push_frame(t, frame(320, 180, n));
        }
    }
    rec.stop(seconds as i64 * 1_000 * MS).unwrap()
}

#[test]
fn every_audio_codec_muxes_in_the_containers_that_allow_it() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("audio-matrix");
    let cases = [
        (AudioCodec::Aac, OutputContainer::Mp4Hybrid),
        (AudioCodec::Aac, OutputContainer::Mkv),
        (AudioCodec::Opus, OutputContainer::Mkv),
        (AudioCodec::Opus, OutputContainer::WebM),
        (AudioCodec::Opus, OutputContainer::Mp4Fragmented),
        (AudioCodec::Flac, OutputContainer::Mkv),
        (AudioCodec::Pcm16, OutputContainer::Mkv),
        (AudioCodec::Pcm24, OutputContainer::Mkv),
    ];
    for (n, (codec_, container)) in cases.into_iter().enumerate() {
        let mut cfg = config(&registry, "libx264", container, 30);
        if container == OutputContainer::WebM {
            cfg = config(&registry, "libvpx_vp9", container, 30);
        }
        cfg.audio = vec![audio_track("Micro", codec_)];
        let summary = record_av(cfg, &dir.join(n.to_string()), 3, 0);
        let seen = demux_audio(&summary.files[0]);
        assert_eq!(seen.streams.len(), 1, "{codec_:?} {container:?}");
        let (_, first, last, title) = &seen.streams[0];
        assert!(
            *first <= 30,
            "{codec_:?} {container:?}: starts at {first} ms"
        );
        assert!(
            (2_900..=3_100).contains(last),
            "{codec_:?} {container:?}: ends at {last} ms"
        );
        // MP4 muxers keep their own handler name; Matroska and WebM carry the title.
        if matches!(container, OutputContainer::Mkv | OutputContainer::WebM) {
            assert_eq!(title.as_deref(), Some("Micro"), "{codec_:?} {container:?}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn surround_tracks_keep_their_channels_in_matroska() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("audio-surround");
    let mut n = 0;
    for codec_ in [
        AudioCodec::Opus,
        AudioCodec::Flac,
        AudioCodec::Pcm16,
        AudioCodec::Pcm24,
    ] {
        assert!(codec_.surround_in(OutputContainer::Mkv));
        for channels in [6usize, 8] {
            let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
            cfg.audio = vec![AudioTrackConfig {
                channels,
                ..audio_track("Jeu", codec_)
            }];
            n += 1;
            let summary = record_av_channels(cfg, &dir.join(n.to_string()), 2, 0, channels);
            let mut input = format::input(&summary.files[0]).unwrap();
            let layouts: Vec<usize> = input
                .streams()
                .filter(|s| s.parameters().medium() == media::Type::Audio)
                .map(|s| {
                    codec::context::Context::from_parameters(s.parameters())
                        .unwrap()
                        .decoder()
                        .audio()
                        .unwrap()
                        .channels() as usize
                })
                .collect();
            assert_eq!(layouts, [channels], "{codec_:?} {channels}");
            let _ = input.packets().count();
        }
    }
    assert!(!AudioCodec::Aac.surround_in(OutputContainer::Mkv));
    assert!(!AudioCodec::Opus.surround_in(OutputContainer::WebM));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn several_tracks_each_get_their_own_named_stream() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("audio-tracks");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.audio = vec![
        audio_track("Micro", AudioCodec::Opus),
        audio_track("Wuthering Waves", AudioCodec::Opus),
        audio_track("Spotify", AudioCodec::Opus),
    ];
    let summary = record_av(cfg, &dir, 2, 0);
    let seen = demux_audio(&summary.files[0]);
    let titles: Vec<_> = seen.streams.iter().map(|s| s.3.clone().unwrap()).collect();
    assert_eq!(titles, ["Micro", "Wuthering Waves", "Spotify"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn audio_and_video_agree_on_time_zero_when_the_first_frame_is_late() {
    // CA-REC-4 (offset part): the first video frame arrives 200 ms after the clock origin; the
    // audio of those 200 ms is cut so that both streams start together.
    let registry = Registry::builtin().unwrap();
    let dir = scratch("audio-sync");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.audio = vec![audio_track("Micro", AudioCodec::Flac)];
    let summary = record_av(cfg, &dir, 4, 200);
    let video = demux(&summary.files[0]);
    let audio = demux_audio(&summary.files[0]);
    let (_, a_first, a_last, _) = audio.streams[0];
    let v_last = *video.pts_ms.last().unwrap() + 33;
    assert!(a_first.abs() <= 25, "audio starts at {a_first}");
    assert!(video.pts_ms[0].abs() <= 1);
    // 4 s of audio minus the 200 ms cut = 3.8 s; the video was fed up to the same instant.
    assert!((3_700..=3_900).contains(&a_last), "audio ends at {a_last}");
    assert!(
        (a_last - v_last).abs() <= 80,
        "audio {a_last} vs video {v_last}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn splitting_keeps_audio_in_every_part() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("audio-split");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.split = Split::Duration(Duration::from_secs(2));
    cfg.audio = vec![audio_track("Micro", AudioCodec::Opus)];
    let summary = record_av(cfg, &dir, 7, 0);
    assert!(summary.files.len() >= 3, "{:?}", summary.files);
    let mut audio_total = 0;
    for file in &summary.files {
        let seen = demux_audio(file);
        let (_, first, last, _) = seen.streams[0];
        assert!(first <= 60, "{file:?}: audio starts at {first}");
        assert!(last > first, "{file:?}");
        audio_total += last - first;
    }
    // Nothing lost beyond the encoder's frame at each cut.
    assert!(
        (6_700..=7_100).contains(&audio_total),
        "{audio_total} ms of audio"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_drifting_source_through_the_mixer_stays_in_sync_with_the_video() {
    // CA-REC-4 (automated part): a fake source whose clock runs 100 ppm fast, mixed by the track
    // mixer, recorded with the video; the audio ends where the recording ends.
    use vixeeny_audio::{FakeAudioSource, Mixer};
    let registry = Registry::builtin().unwrap();
    let dir = scratch("audio-drift");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.audio = vec![audio_track("Micro", AudioCodec::Opus)];
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    let seconds = 120i64;
    let mut source = FakeAudioSource::new(440.0);
    source.drift = 1.0001;
    let mut mixer = Mixer::new(&[1.0], 0);
    let mut t = 0i64;
    let mut n = 0usize;
    while t < seconds * 1_000 * MS {
        let chunk = source.next_chunk(480);
        mixer.push(0, &chunk);
        t += 10 * MS;
        for block in mixer.drain(t) {
            rec.push_audio(0, block.samples);
        }
        if (t / (10 * MS)) % 3 == 0 {
            rec.push_frame(t, frame(320, 180, n));
            n += 1;
        }
    }
    for block in mixer.finish(t) {
        rec.push_audio(0, block.samples);
    }
    let summary = rec.stop(t).unwrap();
    let audio = demux_audio(&summary.files[0]);
    let video = demux(&summary.files[0]);
    let (_, first, last, _) = audio.streams[0];
    let v_end = *video.pts_ms.last().unwrap() + 33;
    assert!(first.abs() <= 30, "audio starts at {first} ms");
    assert!(
        (last - seconds * 1000).abs() <= 40,
        "audio ends at {last} ms"
    );
    assert!(
        (last - v_end).abs() <= 60,
        "audio {last} ms vs video {v_end} ms"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ---- replay buffer ----

/// A replay-only session fed `seconds` of 30 fps video and a stereo tone track.
fn replay_session(
    container: OutputContainer,
    audio: bool,
    keep: u32,
    seconds: usize,
) -> (Recorder, PathBuf) {
    let registry = Registry::builtin().unwrap();
    let dir = scratch(&format!("replay-{}", container.extension()));
    let mut cfg = config(&registry, "libx264", container, 30);
    cfg.files = false;
    cfg.replay_seconds = Some(keep);
    if audio {
        cfg.audio = vec![audio_track("Jeu", AudioCodec::Opus)];
    }
    let ext = cfg.container.extension();
    let rec = Recorder::start(cfg, namer(&dir, ext)).unwrap();
    for (k, block) in tone_blocks(seconds, 0).into_iter().enumerate() {
        if audio {
            rec.push_audio(0, block);
        }
        if k % 2 == 0 {
            rec.push_frame(k as i64 * 20 * MS, frame(320, 180, k / 2));
        }
    }
    (rec, dir)
}

#[test]
fn a_replay_save_holds_the_requested_duration_without_writing_while_buffering() {
    let (rec, dir) = replay_session(OutputContainer::Mkv, true, 5, 12);
    let save = rec.save_replay(dir.join("replay.mkv")).unwrap();
    // 5 s asked, key frames every second: 4 to 5 s of media.
    assert!((4.0..=5.1).contains(&save.seconds), "{}", save.seconds);
    let path = save.wait().unwrap();
    let video = demux(&path);
    assert_eq!(video.pts_ms[0], 0, "starts at zero");
    assert!(video.keyframes >= 4);
    let span = video.pts_ms.last().unwrap() - video.pts_ms[0];
    assert!((3_800..=5_000).contains(&span), "{span} ms");
    let audio = demux_audio(&path);
    assert_eq!(audio.streams.len(), 1);
    assert_eq!(audio.streams[0].3.as_deref(), Some("Jeu"));
    // Audio and video cover the same stretch (to within a packet or two).
    assert!(audio.streams[0].1.abs() < 60, "{}", audio.streams[0].1);
    assert!(
        (audio.streams[0].2 - span).abs() < 200,
        "{:?}",
        audio.streams
    );
    // The buffer keeps running: the session ends with no file of its own.
    let summary = rec.stop(12_000 * MS).unwrap();
    assert!(summary.files.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn two_saves_in_a_row_make_two_valid_files_and_a_short_buffer_gives_what_it_has() {
    let (rec, dir) = replay_session(OutputContainer::Mkv, false, 30, 4);
    let a = rec.save_replay(dir.join("a.mkv")).unwrap();
    let b = rec.save_replay(dir.join("b.mkv")).unwrap();
    let (a, b) = (a.wait().unwrap(), b.wait().unwrap());
    for path in [&a, &b] {
        let seen = demux(path);
        assert_eq!(seen.pts_ms[0], 0);
        // Only ~4 s were buffered: that is what the file has.
        assert!(seen.pts_ms.len() >= 100, "{}", seen.pts_ms.len());
    }
    drop(rec);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_save_before_any_key_frame_is_refused_and_a_plain_recording_has_no_replay() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("replay-empty");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    cfg.files = false;
    cfg.replay_seconds = Some(5);
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    assert!(rec.save_replay(dir.join("x.mkv")).is_err());
    drop(rec);
    let cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    assert!(rec.save_replay(dir.join("y.mkv")).is_err());
    rec.stop(0).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_replay_and_a_recording_share_one_encoder_session() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("replay-shared");
    let mut cfg = config(&registry, "libx264", OutputContainer::Mp4Hybrid, 30);
    cfg.replay_seconds = Some(5);
    let rec = Recorder::start(cfg, namer(&dir, "mp4")).unwrap();
    for i in 0..240 {
        rec.push_frame(i * 1_000 * MS / 30, frame(320, 180, i as usize));
    }
    let path = rec
        .save_replay(dir.join("replay.mp4"))
        .unwrap()
        .wait()
        .unwrap();
    let summary = rec.stop(8_000 * MS).unwrap();
    // The recording is whole, and the replay is a classic MP4 cut from the same packets.
    assert_eq!(demux(&summary.files[0]).pts_ms.len(), 240);
    assert!(
        boxes(&path).iter().position(|b| b == "moov")
            < boxes(&path).iter().position(|b| b == "mdat")
    );
    // Starts where a recording starts (MP4 keeps the encoder delay as an offset).
    assert_eq!(demux(&path).pts_ms[0], demux(&summary.files[0]).pts_ms[0]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_recording_has_a_thumbnail_no_bigger_than_asked() {
    let registry = Registry::builtin().unwrap();
    let dir = scratch("thumbnail");
    let summary = record(
        config(&registry, "libx264", OutputContainer::Mp4Hybrid, 30),
        &dir,
        20,
    );
    let file = summary.files.first().unwrap();
    let (w, h, rgba) = vixeeny_encode::thumbnail::video_thumbnail(file, 160).unwrap();
    assert_eq!((w, h), (160, 90));
    assert_eq!(rgba.len(), 160 * 90 * 4);
    assert!(rgba.iter().any(|b| *b != 0), "the thumbnail is blank");
    assert!(vixeeny_encode::thumbnail::video_thumbnail(&dir.join("missing.mp4"), 160).is_none());
}

#[test]
fn a_replay_kept_on_disk_saves_the_same_file_as_one_kept_in_ram() {
    use vixeeny_encode::replay::Storage;
    let registry = Registry::builtin().unwrap();
    let dir = scratch("replay-disk");
    let buffer = dir.join("buffer");
    let mut seen = Vec::new();
    for (name, storage) in [
        ("ram", Storage::Ram),
        ("disk", Storage::Disk(buffer.clone())),
    ] {
        let mut cfg = config(&registry, "libx264", OutputContainer::Mkv, 30);
        cfg.files = false;
        cfg.replay_seconds = Some(5);
        cfg.replay_storage = storage;
        let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
        for i in 0..240 {
            rec.push_frame(i * 1_000 * MS / 30, frame(320, 180, i as usize));
        }
        let path = rec
            .save_replay(dir.join(format!("{name}.mkv")))
            .unwrap()
            .wait()
            .unwrap();
        seen.push(demux(&path));
        drop(rec);
    }
    // Same packets, same times: the disk is only a place to keep them.
    assert_eq!(seen[0].pts_ms, seen[1].pts_ms);
    assert_eq!(seen[0].keyframes, seen[1].keyframes);
    // The buffer's temporary files are gone with the recorder.
    let left: Vec<_> = std::fs::read_dir(&buffer)
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(left.iter().all(|e| e.path().is_dir()), "{left:?}");
    let _ = std::fs::remove_dir_all(dir);
}
