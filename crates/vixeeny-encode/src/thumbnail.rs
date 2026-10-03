// SPDX-License-Identifier: GPL-3.0-or-later
//! A picture of a video file, for the gallery: the first frame, scaled down.

use std::path::Path;

use ffmpeg_next::{codec, format, frame, media, software::scaling};

/// RGBA pixels of a frame of `path` (width, height, bytes) at most `max_side` pixels on the long
/// side, or `None` when the file has no video or cannot be decoded.
pub fn video_thumbnail(path: &Path, max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    ffmpeg_next::init().ok()?;
    let mut input = format::input(path).ok()?;
    let stream = input.streams().best(media::Type::Video)?;
    let index = stream.index();
    let context = codec::context::Context::from_parameters(stream.parameters()).ok()?;
    let mut decoder = context.decoder().video().ok()?;
    let mut decoded = frame::Video::empty();
    for (s, packet) in input.packets() {
        if s.index() != index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        if decoder.receive_frame(&mut decoded).is_ok() {
            return scale(&decoded, max_side);
        }
    }
    decoder.send_eof().ok()?;
    decoder.receive_frame(&mut decoded).ok()?;
    scale(&decoded, max_side)
}

fn scale(source: &frame::Video, max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (source.width(), source.height());
    if w == 0 || h == 0 {
        return None;
    }
    let ratio = (max_side as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (
        ((w as f32 * ratio).round() as u32).max(1),
        ((h as f32 * ratio).round() as u32).max(1),
    );
    let mut scaler = scaling::Context::get(
        source.format(),
        w,
        h,
        ffmpeg_next::format::Pixel::RGBA,
        tw,
        th,
        scaling::Flags::AREA,
    )
    .ok()?;
    let mut out = frame::Video::empty();
    scaler.run(source, &mut out).ok()?;
    // Rows may be padded: copy them one by one.
    let stride = out.stride(0);
    let data = out.data(0);
    let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
    for row in 0..th as usize {
        rgba.extend_from_slice(data.get(row * stride..row * stride + tw as usize * 4)?);
    }
    Some((tw, th, rgba))
}
