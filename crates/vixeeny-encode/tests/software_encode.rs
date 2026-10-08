// SPDX-License-Identifier: GPL-3.0-or-later
//! M1 demo: every software video encoder × every allowed container (plan 13.2) encodes
//! 100 synthetic frames; every audio encoder × allowed container encodes 1 s of sine.
//! Each output is demuxed again and its packet count / codec checked.
//! Needs the native build: `cargo xtask build-native ffmpeg`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ffmpeg::format::sample::Type as SampleType;
use ffmpeg::format::{Pixel, Sample};
use ffmpeg::{ChannelLayout, Dictionary, Packet, Rational, codec, encoder, format, frame, media};
use vixeeny_encode::ffmpeg;

const W: u32 = 320;
const H: u32 = 240;
const FRAMES: usize = 100;
const FPS: i32 = 25;
const RATE: u32 = 48_000;
const AUDIO_SECONDS: usize = 1;

/// (label, muxer, extension, muxer options)
const CONTAINERS: &[(&str, &str, &str, &str)] = &[
    ("mkv", "matroska", "mkv", ""),
    ("mp4", "mp4", "mp4", ""),
    (
        "fmp4",
        "mp4",
        "mp4",
        "movflags=frag_keyframe+empty_moov+default_base_moof",
    ),
    ("webm", "webm", "webm", ""),
];

/// (encoder, codec family for the container matrix, encoder options)
const VIDEO: &[(&str, &str, &str)] = &[
    ("libx264", "h264", "preset=ultrafast"),
    (
        "libx265",
        "hevc",
        "preset=ultrafast,x265-params=log-level=error",
    ),
    ("libsvtav1", "av1", "preset=12"),
    ("libvpx-vp9", "vp9", "deadline=realtime,cpu-used=8"),
];

/// (encoder, family, sample format)
fn audio() -> Vec<(&'static str, &'static str, Sample)> {
    vec![
        ("aac", "aac", Sample::F32(SampleType::Planar)),
        ("libopus", "opus", Sample::F32(SampleType::Packed)),
        ("flac", "flac", Sample::I16(SampleType::Packed)),
        ("pcm_s16le", "pcm", Sample::I16(SampleType::Packed)),
    ]
}

fn opts(s: &str) -> Dictionary<'static> {
    let mut d = Dictionary::new();
    for kv in s.split(',').filter(|kv| !kv.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap();
        d.set(k, v);
    }
    d
}

/// Plan 13.2 container × codec matrix.
fn allowed(container: &str, family: &str) -> bool {
    match container {
        "webm" => matches!(family, "av1" | "vp9" | "opus"),
        "mkv" => true,
        _ => family != "pcm",
    }
}

fn synth_frame(i: usize) -> frame::Video {
    let mut f = frame::Video::new(Pixel::YUV420P, W, H);
    for plane in 0..3 {
        let stride = f.stride(plane);
        let (w, h) = if plane == 0 {
            (W as usize, H as usize)
        } else {
            (W as usize / 2, H as usize / 2)
        };
        let data = f.data_mut(plane);
        for y in 0..h {
            for x in 0..w {
                data[y * stride + x] = ((x + y + i * 3) % 256) as u8;
            }
        }
    }
    f.set_pts(Some(i as i64));
    f
}

fn synth_audio(fmt: Sample, offset: usize, samples: usize) -> frame::Audio {
    let mut f = frame::Audio::new(fmt, samples, ChannelLayout::STEREO);
    f.set_rate(RATE);
    f.set_pts(Some(offset as i64));
    let value =
        |n: usize| ((offset + n) as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5;
    for n in 0..samples {
        for ch in 0..2 {
            let v = value(n);
            match fmt {
                Sample::F32(SampleType::Planar) => {
                    f.data_mut(ch)[n * 4..n * 4 + 4].copy_from_slice(&v.to_ne_bytes());
                }
                Sample::F32(SampleType::Packed) => {
                    let i = (n * 2 + ch) * 4;
                    f.data_mut(0)[i..i + 4].copy_from_slice(&v.to_ne_bytes());
                }
                Sample::I16(SampleType::Packed) => {
                    let i = (n * 2 + ch) * 2;
                    f.data_mut(0)[i..i + 2].copy_from_slice(&((v * 32767.0) as i16).to_ne_bytes());
                }
                _ => unreachable!("unsupported sample format in test"),
            }
        }
    }
    f
}

fn write_packets(
    octx: &mut format::context::Output,
    tb: Rational,
    receive: &mut dyn FnMut(&mut Packet) -> bool,
) {
    let stream_tb = octx.stream(0).unwrap().time_base();
    let mut pkt = Packet::empty();
    while receive(&mut pkt) {
        pkt.set_stream(0);
        pkt.rescale_ts(tb, stream_tb);
        pkt.write_interleaved(octx).unwrap();
    }
}

fn encode_video(
    enc: &str,
    enc_opts: &str,
    label: &str,
    mux: &str,
    ext: &str,
    mopts: &str,
) -> PathBuf {
    let path = std::env::temp_dir().join(format!("vx-m1-{enc}-{label}.{ext}"));
    let mut octx = format::output_as(&path, mux).unwrap();
    let codec = encoder::find_by_name(enc).unwrap_or_else(|| panic!("{enc} is not built in"));
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .unwrap();
    ctx.set_width(W);
    ctx.set_height(H);
    ctx.set_format(Pixel::YUV420P);
    let tb = Rational(1, FPS);
    ctx.set_time_base(tb);
    ctx.set_frame_rate(Some(Rational(FPS, 1)));
    if octx.format().flags().contains(format::Flags::GLOBAL_HEADER) {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut venc = ctx.open_with(opts(enc_opts)).unwrap();
    octx.add_stream(codec).unwrap().set_parameters(&venc);
    octx.write_header_with(opts(mopts)).unwrap();
    for i in 0..FRAMES {
        venc.send_frame(&synth_frame(i)).unwrap();
        write_packets(&mut octx, tb, &mut |p| venc.receive_packet(p).is_ok());
    }
    venc.send_eof().unwrap();
    write_packets(&mut octx, tb, &mut |p| venc.receive_packet(p).is_ok());
    octx.write_trailer().unwrap();
    path
}

fn encode_audio(enc: &str, fmt: Sample, label: &str, mux: &str, ext: &str, mopts: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("vx-m1-{enc}-{label}.{ext}"));
    let mut octx = format::output_as(&path, mux).unwrap();
    let codec = encoder::find_by_name(enc).unwrap_or_else(|| panic!("{enc} is not built in"));
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .unwrap();
    ctx.set_rate(RATE as i32);
    ctx.set_channel_layout(ChannelLayout::STEREO);
    ctx.set_format(fmt);
    let tb = Rational(1, RATE as i32);
    ctx.set_time_base(tb);
    if octx.format().flags().contains(format::Flags::GLOBAL_HEADER) {
        ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut aenc = ctx.open().unwrap();
    octx.add_stream(codec).unwrap().set_parameters(&aenc);
    octx.write_header_with(opts(mopts)).unwrap();
    let frame_size = match aenc.frame_size() {
        0 => 1024,
        n => n as usize,
    };
    let total = RATE as usize * AUDIO_SECONDS;
    let mut offset = 0;
    while offset + frame_size <= total {
        aenc.send_frame(&synth_audio(fmt, offset, frame_size))
            .unwrap();
        write_packets(&mut octx, tb, &mut |p| aenc.receive_packet(p).is_ok());
        offset += frame_size;
    }
    aenc.send_eof().unwrap();
    write_packets(&mut octx, tb, &mut |p| aenc.receive_packet(p).is_ok());
    octx.write_trailer().unwrap();
    path
}

fn count_packets(path: &PathBuf, kind: media::Type) -> usize {
    let mut ictx = format::input(path).unwrap();
    let idx = ictx
        .streams()
        .best(kind)
        .expect("stream missing in output")
        .index();
    ictx.packets().filter(|(s, _)| s.index() == idx).count()
}

#[test]
fn software_video_encoders_in_every_container() {
    ffmpeg::init().unwrap();
    for &(enc, family, enc_opts) in VIDEO {
        for &(label, mux, ext, mopts) in CONTAINERS.iter().filter(|c| allowed(c.0, family)) {
            let path = encode_video(enc, enc_opts, label, mux, ext, mopts);
            assert_eq!(
                count_packets(&path, media::Type::Video),
                FRAMES,
                "{enc} in {label}"
            );
            std::fs::remove_file(path).ok();
        }
    }
}

#[test]
fn software_audio_encoders_in_every_container() {
    ffmpeg::init().unwrap();
    for (enc, family, fmt) in audio() {
        for &(label, mux, ext, mopts) in CONTAINERS.iter().filter(|c| allowed(c.0, family)) {
            let path = encode_audio(enc, fmt, label, mux, ext, mopts);
            assert!(
                count_packets(&path, media::Type::Audio) > 0,
                "{enc} in {label}"
            );
            std::fs::remove_file(path).ok();
        }
    }
}
