// SPDX-License-Identifier: GPL-3.0-or-later
//! The encoder settings of the registry against the real encoders: the custom preset of each
//! encoder in every rate mode, every declared pixel format and every option value (the hardware
//! encoders when this machine has them). The recordings themselves are tested in `recorder.rs`.
//! Needs the native build: `cargo xtask build-native ffmpeg`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ffmpeg::format::Pixel;
use ffmpeg::{Dictionary, Packet, Rational, codec, encoder, format, frame, media};
use vixeeny_encode::ffmpeg;

const W: u32 = 320;
const H: u32 = 240;
const FRAMES: usize = 100;
const FPS: i32 = 25;

/// (encoder, options that keep it fast)
const VIDEO: &[(&str, &str)] = &[
    ("libx264", "preset=ultrafast"),
    ("libx265", "preset=ultrafast,x265-params=log-level=error"),
    ("libsvtav1", "preset=12"),
    ("libvpx-vp9", "deadline=realtime,cpu-used=8"),
];

fn opts(s: &str) -> Dictionary<'static> {
    let mut d = Dictionary::new();
    for kv in s.split(',').filter(|kv| !kv.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap();
        d.set(k, v);
    }
    d
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

fn count_packets(path: &PathBuf, kind: media::Type) -> usize {
    let mut ictx = format::input(path).unwrap();
    let idx = ictx
        .streams()
        .best(kind)
        .expect("stream missing in output")
        .index();
    ictx.packets().filter(|(s, _)| s.index() == idx).count()
}

/// The custom preset of every software encoder opens and encodes in each of its rate modes.
#[test]
fn custom_rate_modes_encode() {
    ffmpeg::init().unwrap();
    let registry = vixeeny_encode::registry::Registry::builtin().unwrap();
    let software = registry
        .encoders()
        .filter(|e| e.kind == vixeeny_encode::registry::Kind::Software);
    for e in software {
        for mode in e.rate_modes() {
            let params = [("rc.mode".to_owned(), mode.name().to_owned())].into();
            let options: Vec<String> = e
                .custom_options(&params)
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            let label = format!("custom-{}", mode.name());
            let path = encode_video(
                &e.ffmpeg_encoder,
                &options.join(","),
                &label,
                "matroska",
                "mkv",
                "",
            );
            assert_eq!(
                count_packets(&path, media::Type::Video),
                FRAMES,
                "{} {mode:?}: {options:?}",
                e.id
            );
            std::fs::remove_file(path).ok();
        }
    }
}

/// Opens `enc` with `options` on NV12 frames (or 4:2:0 planes, for an encoder without NV12)
/// and encodes a few: `Err` when it refuses.
fn try_encode(enc: &str, options: &str) -> Result<usize, ffmpeg::Error> {
    let codec = encoder::find_by_name(enc).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let nv12 = codec
        .video()
        .ok()
        .and_then(|v| v.formats())
        .is_none_or(|mut f| f.any(|p| p == Pixel::NV12));
    let format = if nv12 { Pixel::NV12 } else { Pixel::YUV420P };
    let mut ctx = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    ctx.set_width(W);
    ctx.set_height(H);
    ctx.set_format(format);
    ctx.set_time_base(Rational(1, FPS));
    ctx.set_frame_rate(Some(Rational(FPS, 1)));
    let mut venc = ctx.open_with(opts(options))?;
    let mut packets = 0;
    let mut pkt = Packet::empty();
    for i in 0..30 {
        let mut f = frame::Video::new(format, W, H);
        for plane in 0..f.planes() {
            f.data_mut(plane).fill((i * 7 % 256) as u8);
        }
        f.set_pts(Some(i as i64));
        venc.send_frame(&f)?;
        while venc.receive_packet(&mut pkt).is_ok() {
            packets += 1;
        }
    }
    venc.send_eof()?;
    while venc.receive_packet(&mut pkt).is_ok() {
        packets += 1;
    }
    Ok(packets)
}

/// The same with the hardware encoders this machine has (the others are skipped).
#[test]
fn custom_rate_modes_on_this_gpu() {
    ffmpeg::init().unwrap();
    let registry = vixeeny_encode::registry::Registry::builtin().unwrap();
    let join = |o: std::collections::BTreeMap<String, String>| {
        o.into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let hardware = registry
        .encoders()
        .filter(|e| e.kind == vixeeny_encode::registry::Kind::Hardware);
    for e in hardware {
        let balanced = e
            .presets
            .balanced
            .iter()
            .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
            .collect();
        if let Err(err) = try_encode(&e.ffmpeg_encoder, &join(balanced)) {
            eprintln!("{}: skipped ({err})", e.id);
            continue;
        }
        for mode in e.rate_modes() {
            let params = [("rc.mode".to_owned(), mode.name().to_owned())].into();
            let options = join(e.custom_options(&params));
            let packets = try_encode(&e.ffmpeg_encoder, &options)
                .unwrap_or_else(|err| panic!("{} {mode:?} ({options}): {err}", e.id));
            eprintln!("{} {mode:?}: {packets} packets", e.id);
            assert_eq!(packets, 30, "{} {mode:?}", e.id);
        }
    }
}

/// The probe does not try software encoders: each one offers every pixel format the registry
/// declares, so each of them must encode.
#[test]
fn software_encoders_encode_every_declared_pixel_format() {
    let _ = ffmpeg::init();
    let registry = vixeeny_encode::registry::Registry::builtin().unwrap();
    let software = registry
        .encoders()
        .filter(|e| e.kind == vixeeny_encode::registry::Kind::Software);
    for e in software {
        let options = VIDEO
            .iter()
            .find(|(name, ..)| *name == e.ffmpeg_encoder)
            .map_or("", |v| v.1);
        for spec in &e.pixel_formats {
            let pix: Pixel = spec.ffmpeg.parse().unwrap();
            let codec = encoder::find_by_name(&e.ffmpeg_encoder).unwrap();
            let mut ctx = codec::context::Context::new_with_codec(codec)
                .encoder()
                .video()
                .unwrap();
            ctx.set_width(W);
            ctx.set_height(H);
            ctx.set_format(pix);
            ctx.set_time_base(Rational(1, FPS));
            ctx.set_frame_rate(Some(Rational(FPS, 1)));
            let label = format!("{} {}", e.id, spec.ffmpeg);
            let mut venc = ctx
                .open_with(opts(options))
                .unwrap_or_else(|err| panic!("{label}: {err}"));
            let mut packets = 0;
            let mut pkt = Packet::empty();
            for i in 0..10 {
                // Zero is a valid sample at every depth (an uninitialised buffer is not).
                let mut f = frame::Video::new(pix, W, H);
                for plane in 0..f.planes() {
                    f.data_mut(plane).fill(0);
                }
                f.set_pts(Some(i));
                venc.send_frame(&f)
                    .unwrap_or_else(|err| panic!("{label}: {err}"));
                while venc.receive_packet(&mut pkt).is_ok() {
                    packets += 1;
                }
            }
            venc.send_eof().unwrap();
            while venc.receive_packet(&mut pkt).is_ok() {
                packets += 1;
            }
            assert!(packets > 0, "{label}: no packet");
        }
    }
}

/// Every value of every expert option (each choice, both ends of a range) opens and encodes,
/// on the software encoders and the hardware ones this machine has.
#[test]
fn every_option_value_encodes() {
    use vixeeny_encode::registry::ParamType;
    ffmpeg::init().unwrap();
    let registry = vixeeny_encode::registry::Registry::builtin().unwrap();
    let join = |o: std::collections::BTreeMap<String, String>| {
        o.into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut failed = Vec::new();
    for e in registry.encoders() {
        if let Err(err) = try_encode(&e.ffmpeg_encoder, &join(e.custom_options(&[].into()))) {
            assert!(
                e.kind == vixeeny_encode::registry::Kind::Hardware,
                "{}: {err}",
                e.id
            );
            eprintln!("{}: skipped ({err})", e.id);
            continue;
        }
        for p in &e.params {
            let values: Vec<String> = match p.kind {
                ParamType::Enum => p.values.clone(),
                ParamType::Bool => vec!["0".into(), "1".into()],
                ParamType::Int | ParamType::Float => [p.min, p.max]
                    .into_iter()
                    .flatten()
                    .map(|v| v.to_string())
                    .collect(),
                ParamType::String => Vec::new(),
            };
            for v in values {
                let params = [(p.key.clone(), v.clone())].into();
                let options = join(e.custom_options(&params));
                match try_encode(&e.ffmpeg_encoder, &options) {
                    Ok(30) => {}
                    Ok(n) => failed.push(format!("{} {}={v}: {n} packets", e.id, p.key)),
                    Err(err) => failed.push(format!("{} {}={v}: {err}", e.id, p.key)),
                }
            }
        }
        eprintln!("{}: every option tried", e.id);
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
