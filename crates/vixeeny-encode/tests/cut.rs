// SPDX-License-Identifier: GPL-3.0-or-later
//! Cutting a recording to the frame: the list of frames, the preview's pictures and sound, and
//! the two ways of writing a part (packets copied from a key frame, or the video encoded again).
//! Needs the native build: `cargo xtask build-native ffmpeg`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vixeeny_encode::audio::{AudioCodec, AudioTrackConfig};
use vixeeny_encode::clock::Fps;
use vixeeny_encode::cut::{Clip, Pictures, Reencode, Sound, export};
use vixeeny_encode::recorder::{
    FrameFormat, OutputContainer, RecordConfig, Recorder, Split, VideoFrame,
};
use vixeeny_encode::registry::{Chroma, PresetName, Registry};

const MS: i64 = 1_000_000;
const FPS: usize = 30;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vixeeny-cut-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Frame `i`: its number drawn as the brightness of the whole picture, so that a decoded frame
/// tells which one it is.
fn frame(i: usize) -> VideoFrame {
    let v = (i * 2 % 256) as u8;
    VideoFrame {
        width: 320,
        height: 180,
        stride: 320 * 4,
        format: FrameFormat::Bgra8,
        data: [v, v, v, 255].repeat(320 * 180),
    }
}

fn x264(registry: &Registry) -> (vixeeny_encode::registry::Encoder, Vec<(String, String)>) {
    let encoder = registry.get("libx264").unwrap().clone();
    let options = encoder
        .presets
        .get(PresetName::Performance)
        .iter()
        .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
        .collect();
    (encoder, options)
}

/// Records `seconds` at 30 fps, a key frame every second, with two audio tracks.
fn recording(dir: &Path, seconds: usize) -> PathBuf {
    recording_with(dir, seconds, false)
}

/// The same, in HDR (x265, 10 bits, BT.2020 PQ) when `hdr`: every frame the reference white.
fn recording_with(dir: &Path, seconds: usize, hdr: bool) -> PathBuf {
    let registry = Registry::builtin().unwrap();
    let (encoder, options) = if hdr {
        let encoder = registry.get("libx265").unwrap().clone();
        let options = encoder
            .presets
            .get(PresetName::Performance)
            .iter()
            .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
            .collect();
        (encoder, options)
    } else {
        x264(&registry)
    };
    let track = |title: &str| AudioTrackConfig {
        title: title.into(),
        codec: AudioCodec::Aac,
        bitrate_kbps: 128,
        vbr: true,
        channels: 2,
    };
    let cfg = RecordConfig {
        crop: None,
        encoder,
        options,
        container: OutputContainer::Mp4Hybrid,
        output_size: (320, 180),
        fps: Fps::whole(FPS as u32),
        depth: if hdr { 10 } else { 8 },
        chroma: Chroma::C420,
        hdr,
        split: Split::Off,
        keyframe_seconds: 1.0,
        queue: 1_000,
        vfr: false,
        audio: vec![track("Game"), track("Mic")],
        gpu: None,
        replay_seconds: None,
        replay_storage: vixeeny_encode::replay::Storage::Ram,
        files: true,
    };
    let out = dir.join("rec.mp4");
    let path = out.clone();
    let rec = Recorder::start(cfg, Box::new(move |_| path.clone())).unwrap();
    // 20 ms blocks of a tone; a video frame every 33 ms.
    let mut next_frame = 0;
    for k in 0..seconds * 50 {
        let t = k as i64 * 20 * MS;
        let block: Vec<f32> = (0..960)
            .flat_map(|i| {
                let v = ((k * 960 + i) as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin();
                [v * 0.5, v * 0.5]
            })
            .collect();
        rec.push_audio(0, block.clone());
        rec.push_audio(1, block);
        while (next_frame as i64) * 1_000 * MS / FPS as i64 <= t {
            let picture = if hdr { white_hdr() } else { frame(next_frame) };
            rec.push_frame(next_frame as i64 * 1_000 * MS / FPS as i64, picture);
            next_frame += 1;
        }
    }
    rec.stop(seconds as i64 * 1_000 * MS).unwrap();
    out
}

/// An scRGB frame at 2.5 (200 nits, about the reference white of HDR).
fn white_hdr() -> VideoFrame {
    let px: Vec<u8> = [0x4100_u16, 0x4100, 0x4100, 0x3C00]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    VideoFrame {
        width: 320,
        height: 180,
        stride: 320 * 8,
        format: FrameFormat::ScRgbHalf,
        data: px.repeat(320 * 180),
    }
}

/// The frame number a picture shows (its brightness).
fn number(bgra: &[u8]) -> usize {
    let sum: u64 = bgra.iter().step_by(4).map(|&b| u64::from(b)).sum();
    let mean = sum as f64 / (bgra.len() / 4) as f64;
    // Limited-range YUV and back: about 16..235 squeezed back to 0..255.
    (mean / 2.0).round() as usize
}

fn reencode() -> Reencode {
    let registry = Registry::builtin().unwrap();
    let (encoder, options) = x264(&registry);
    Reencode {
        encoder,
        options,
        depth: 8,
    }
}

#[test]
fn the_clip_lists_its_frames_and_key_frames() {
    let dir = scratch("list");
    let clip = Clip::open(&recording(&dir, 3)).unwrap();
    assert_eq!(clip.frames.len(), 3 * FPS);
    assert_eq!((clip.width, clip.height), (320, 180));
    assert_eq!(clip.audio_tracks, 2);
    let keys: Vec<usize> = (0..clip.frames.len())
        .filter(|&i| clip.frames[i].key)
        .collect();
    assert_eq!(keys, [0, 30, 60]);
    assert!((clip.duration() - 3.0).abs() < 0.05, "{}", clip.duration());
    assert_eq!(clip.frame_at(1.5), 45);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_preview_decodes_the_frame_asked_for_and_the_ones_after() {
    let dir = scratch("preview");
    let clip = Clip::open(&recording(&dir, 3)).unwrap();
    let mut pictures = Pictures::open(&clip).unwrap();
    for target in [45, 10, 89, 0, 31] {
        pictures.seek(target).unwrap();
        let (i, picture) = pictures.read((160, 90)).unwrap().unwrap();
        assert_eq!(i, target);
        assert_eq!((picture.width, picture.height), (160, 90));
        assert_eq!(picture.bgra.len(), 160 * 90 * 4);
        let shown = number(&picture.bgra);
        assert!(shown.abs_diff(target) <= 1, "frame {target} shows {shown}");
        if target < 89 {
            assert_eq!(pictures.read((160, 90)).unwrap().unwrap().0, target + 1);
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_graphics_card_and_the_processor_show_the_same_frames() {
    let dir = scratch("hardware");
    let clip = Clip::open(&recording(&dir, 2)).unwrap();
    let mut card = Pictures::open(&clip).unwrap();
    let mut cpu = Pictures::open_with(&clip, false).unwrap();
    assert!(!cpu.hardware());
    eprintln!("decoding on the graphics card: {}", card.hardware());
    for target in [0, 37, 12, 59] {
        card.seek(target).unwrap();
        cpu.seek(target).unwrap();
        let (a, pa) = card.read((160, 90)).unwrap().unwrap();
        let (b, pb) = cpu.read((160, 90)).unwrap().unwrap();
        assert_eq!((a, b), (target, target));
        assert!(number(&pa.bgra).abs_diff(number(&pb.bgra)) <= 1);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_hdr_clip_is_previewed_in_sdr() {
    let dir = scratch("hdr");
    let clip = Clip::open(&recording_with(&dir, 1, true)).unwrap();
    assert!(clip.hdr && clip.deep);
    for hardware in [true, false] {
        let mut pictures = Pictures::open_with(&clip, hardware).unwrap();
        pictures.seek(10).unwrap();
        let (_, picture) = pictures.read((160, 90)).unwrap().unwrap();
        let centre = &picture.bgra[(45 * 160 + 80) * 4..][..4];
        // Shown as PQ codes it would be a mid grey (≈ 150): tone mapped, a near white.
        assert!(
            centre[..3].iter().all(|&c| (225..=250).contains(&c)),
            "{centre:?} (card: {})",
            pictures.hardware()
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_preview_sound_mixes_the_tracks_from_where_it_is_asked() {
    let dir = scratch("sound");
    let clip = Clip::open(&recording(&dir, 3)).unwrap();
    let mut sound = Sound::open(&clip).unwrap().unwrap();
    sound.seek(1.0).unwrap();
    let mut total = 0;
    let mut peak = 0.0_f32;
    while let Some(block) = sound.read().unwrap() {
        assert_eq!(block.len() % 2, 0);
        total += block.len() / 2;
        peak = block.iter().fold(peak, |p, v| p.max(v.abs()));
    }
    // About two seconds left, the two tracks added (each at half volume).
    let seconds = total as f64 / 48_000.0;
    assert!((seconds - 2.0).abs() < 0.1, "{seconds} s");
    assert!(peak > 0.7, "peak {peak}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_part_from_a_key_frame_is_copied_and_another_one_encoded_again() {
    let dir = scratch("export");
    let clip = Clip::open(&recording(&dir, 3)).unwrap();
    let never = AtomicBool::new(false);

    // From a key frame: the packets as they are, no encoder needed.
    assert!(clip.lossless(30, 59));
    let copied = dir.join("copied.mp4");
    export(&clip, (30, 59), &copied, None, &|_| {}, &never).unwrap();
    let part = Clip::open(&copied).unwrap();
    assert_eq!(part.frames.len(), 30);
    assert_eq!(part.audio_tracks, 2);
    let mut pictures = Pictures::open(&part).unwrap();
    pictures.seek(0).unwrap();
    let (_, first) = pictures.read((32, 18)).unwrap().unwrap();
    assert!(number(&first.bgra).abs_diff(30) <= 1);

    // Between two key frames: refused without an encoder, then encoded again.
    assert!(!clip.lossless(10, 40));
    let encoded = dir.join("encoded.mp4");
    assert!(export(&clip, (10, 40), &encoded, None, &|_| {}, &never).is_err());
    let last = std::cell::Cell::new(0.0);
    export(
        &clip,
        (10, 40),
        &encoded,
        Some(&reencode()),
        &|p| last.set(p),
        &never,
    )
    .unwrap();
    assert!((last.get() - 1.0).abs() < f32::EPSILON);
    let part = Clip::open(&encoded).unwrap();
    assert_eq!(part.frames.len(), 31);
    assert!(part.frames[0].key);
    assert!((part.time(0)).abs() < 1e-9);
    let mut pictures = Pictures::open(&part).unwrap();
    pictures.seek(0).unwrap();
    let (_, first) = pictures.read((32, 18)).unwrap().unwrap();
    assert!(
        number(&first.bgra).abs_diff(10) <= 1,
        "first frame shows {}",
        number(&first.bgra)
    );
    pictures.seek(30).unwrap();
    let (_, end) = pictures.read((32, 18)).unwrap().unwrap();
    assert!(number(&end.bgra).abs_diff(40) <= 1);
    // The sound follows: about a second, from the cut on.
    let mut sound = Sound::open(&part).unwrap().unwrap();
    let mut total = 0;
    while let Some(block) = sound.read().unwrap() {
        total += block.len() / 2;
    }
    let seconds = total as f64 / 48_000.0;
    assert!((seconds - 31.0 / 30.0).abs() < 0.1, "{seconds} s");

    // A cancelled export leaves nothing behind.
    let cancelled = dir.join("cancelled.mp4");
    let stop = AtomicBool::new(true);
    assert!(
        export(
            &clip,
            (10, 40),
            &cancelled,
            Some(&reencode()),
            &|_| {},
            &stop
        )
        .is_err()
    );
    assert!(!cancelled.exists());
    assert!(!cancelled.with_extension("part.tmp").exists());
    let _ = std::fs::remove_dir_all(dir);
}
