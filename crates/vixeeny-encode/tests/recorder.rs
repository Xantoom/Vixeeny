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
use vixeeny_encode::recorder::{OutputContainer, RecordConfig, Recorder, Split, VideoFrame};
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
        bgra,
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
    cfg.queue = 2;
    cfg.output_size = (1280, 720);
    // A slow preset on a big frame, fed as fast as possible.
    cfg.options = vec![
        ("preset".into(), "veryslow".into()),
        ("crf".into(), "18".into()),
    ];
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    let started = std::time::Instant::now();
    let mut refused = 0;
    for i in 0..200 {
        if !rec.push_frame(i * 33 * MS, frame(1280, 720, i as usize)) {
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
