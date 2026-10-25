// SPDX-License-Identifier: GPL-3.0-or-later
//! The GPU path end to end (Windows): a solid colour texture is converted by the D3D11 video
//! processor, handed to the encoder (downloaded for a software encoder, as it is for NVENC when
//! this machine has one) and the decoded file holds the BT.709 limited-range values of that
//! colour, like the CPU path.
//! Needs the native build: `cargo xtask build-native ffmpeg`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;
use std::sync::Arc;

use vixeeny_encode::clock::Fps;
use vixeeny_encode::d3d_convert::{self as d3d, Device, DeviceContext, Rect};
use vixeeny_encode::ffmpeg::{codec, format, frame, media};
use vixeeny_encode::gpu::{Feed, GpuPipeline, Source};
use vixeeny_encode::recorder::{FrameFormat, RecordConfig, Recorder, VideoFrame};
use vixeeny_encode::registry::{Chroma, PresetName, Registry};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use common::{namer, scratch};

const W: u32 = 320;
const H: u32 = 180;
const MS: i64 = 1_000_000;
/// Orange: B, G, R, A.
const COLOUR: [u8; 4] = [40, 160, 220, 255];
/// Its BT.709 limited-range Y, Cb, Cr.
const YCBCR: [i32; 3] = [157, 69, 159];

fn texture(device: &Device) -> ID3D11Texture2D {
    let pixels: Vec<u8> = COLOUR.repeat((W * H) as usize);
    let bind = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
    let format = DXGI_FORMAT_B8G8R8A8_UNORM;
    vixeeny_platform::d3d::texture(device, (W, H), format, bind, 0, Some((&pixels, W * 4))).unwrap()
}

/// Y, Cb, Cr of the centre of the first decoded frame, on 8 bits.
fn centre(path: &Path) -> [i32; 3] {
    let mut input = format::input(path).unwrap();
    let index = input.streams().best(media::Type::Video).unwrap().index();
    let params = input.stream(index).unwrap().parameters();
    let mut decoder = codec::context::Context::from_parameters(params)
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let mut decoded = frame::Video::empty();
    for (s, packet) in input.packets() {
        if s.index() != index {
            continue;
        }
        decoder.send_packet(&packet).unwrap();
        if decoder.receive_frame(&mut decoded).is_ok() {
            break;
        }
    }
    let (x, y) = (W as usize / 2, H as usize / 2);
    use vixeeny_encode::ffmpeg::format::Pixel;
    let ten = matches!(
        decoded.format(),
        Pixel::YUV420P10LE | Pixel::YUV422P10LE | Pixel::YUV444P10LE
    );
    // Chroma samples per luma sample, across and down.
    let (cx, cy) = match decoded.format() {
        Pixel::YUV444P | Pixel::YUV444P10LE => (1, 1),
        Pixel::YUV422P | Pixel::YUV422P10LE => (2, 1),
        _ => (2, 2),
    };
    let sample = |plane: usize, x: usize, y: usize| {
        let data = decoded.data(plane);
        let row = y * decoded.stride(plane);
        if ten {
            i32::from(u16::from_le_bytes([
                data[row + x * 2],
                data[row + x * 2 + 1],
            ])) / 4
        } else {
            i32::from(data[row + x])
        }
    };
    [
        sample(0, x, y),
        sample(1, x / cx, y / cy),
        sample(2, x / cx, y / cy),
    ]
}

fn assert_colour(got: [i32; 3], what: &str) {
    for (g, want) in got.iter().zip(YCBCR) {
        assert!((g - want).abs() <= 4, "{what}: got {got:?}, want {YCBCR:?}");
    }
}

/// Records 10 frames of the orange texture through the GPU path of `id`. `None` when this
/// machine's adapter has no video processor (the build servers' software one).
fn record_gpu(
    device: &Device,
    context: &DeviceContext,
    id: &str,
    feed: Feed,
    depth: u8,
    chroma: Chroma,
) -> Option<[i32; 3]> {
    record_gpu_with(
        device,
        context,
        id,
        feed,
        (depth, chroma),
        PresetName::Performance,
    )
}

fn record_gpu_with(
    device: &Device,
    context: &DeviceContext,
    id: &str,
    feed: Feed,
    (depth, chroma): (u8, Chroma),
    preset: PresetName,
) -> Option<[i32; 3]> {
    let registry = Registry::builtin().unwrap();
    let pipeline = match GpuPipeline::new(
        device,
        context,
        Source {
            size: (W, H),
            hdr: None,
        },
        (W, H),
        Fps::whole(30),
        false,
        depth == 10,
        chroma,
        feed,
    ) {
        Ok(p) => p,
        // Beyond 4:2:0, what the video processor writes depends on the driver.
        Err(e) if chroma != Chroma::C420 && e.to_string().contains("cannot write") => {
            eprintln!("{id} {}: {e}", chroma.name());
            return None;
        }
        Err(e) => {
            vixeeny_common::skip_test(format!("no GPU video processor ({e})"));
            return None;
        }
    };
    let cfg = RecordConfig {
        depth,
        chroma,
        ..common::config(&registry, id, preset)
    };
    let name = format!("{id}-{depth}-{}-{preset:?}", chroma.name());
    Some(record_texture(cfg, pipeline, &texture(device), &name))
}

/// Records 10 frames of `source` through `pipeline`: Y, Cb, Cr of the centre of the first one.
fn record_texture(
    mut cfg: RecordConfig,
    pipeline: GpuPipeline,
    source: &ID3D11Texture2D,
    name: &str,
) -> [i32; 3] {
    let pipeline = Arc::new(pipeline);
    cfg.gpu = Some(Arc::clone(&pipeline));
    let dir = scratch(name);
    let rec = Recorder::start(cfg, namer(&dir, "mkv")).unwrap();
    let rect = Rect {
        left: 0,
        top: 0,
        right: W as i32,
        bottom: H as i32,
    };
    for i in 0..10 {
        let frame = pipeline.convert(source, rect).unwrap();
        assert!(rec.push_hw(i * 1_000 * MS / 30, frame));
    }
    let summary = rec.stop(10 * 1_000 * MS / 30).unwrap();
    centre(&summary.files[0])
}

#[test]
fn a_software_encoder_records_downloaded_gpu_frames_with_the_right_colours() {
    let (device, context) = d3d::create_device(None).unwrap();
    let Some(got) = record_gpu(
        &device,
        &context,
        "libx264",
        Feed::Download,
        8,
        Chroma::C420,
    ) else {
        return;
    };
    assert_colour(got, "x264 via the GPU");
    // The CPU path gives the same colour.
    let registry = Registry::builtin().unwrap();
    let dir = scratch("cpu");
    let rec = Recorder::start(
        common::config(&registry, "libx264", PresetName::Performance),
        namer(&dir, "mkv"),
    )
    .unwrap();
    for i in 0..10 {
        let frame = VideoFrame {
            width: W,
            height: H,
            stride: (W * 4) as usize,
            format: FrameFormat::Bgra8,
            data: COLOUR.repeat((W * H) as usize),
        };
        assert!(rec.push_frame(i * 1_000 * MS / 30, frame));
    }
    let summary = rec.stop(10 * 1_000 * MS / 30).unwrap();
    assert_colour(centre(&summary.files[0]), "x264 via the CPU");
}

/// Whether `device` is on an NVIDIA card (machines without one fall back to the default adapter).
fn is_nvidia(device: &Device) -> bool {
    use windows::core::Interface;
    let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast().unwrap();
    // SAFETY: plain queries of a live device.
    let adapter = unsafe { dxgi.GetAdapter() }.unwrap();
    // SAFETY: as above.
    unsafe { adapter.GetDesc() }.unwrap().VendorId == 0x10de
}

#[test]
fn nvenc_reads_the_gpu_frames_directly_when_present() {
    let Ok((device, context)) = d3d::create_device(Some(0x10de)) else {
        vixeeny_common::skip_test("no Direct3D device");
        return;
    };
    let registry = Registry::builtin().unwrap();
    let probe_ok = vixeeny_encode::ffmpeg::encoder::find_by_name("h264_nvenc").is_some()
        && registry.get("nvenc_h264").is_some();
    if !probe_ok {
        vixeeny_common::skip_test("no NVENC in FFmpeg");
        return;
    }
    if !is_nvidia(&device) {
        vixeeny_common::skip_test("no NVIDIA card");
        return;
    }
    assert_colour(
        record_gpu(
            &device,
            &context,
            "nvenc_h264",
            Feed::Direct,
            8,
            Chroma::C420,
        )
        .unwrap(),
        "NVENC via the GPU",
    );
    // Downloaded frames into a hardware encoder: QSV's fallback, tried on NVENC.
    assert_colour(
        record_gpu(
            &device,
            &context,
            "nvenc_h264",
            Feed::Download,
            8,
            Chroma::C420,
        )
        .unwrap(),
        "NVENC from downloaded frames",
    );
}

/// The quality presets ask for B-frames as references, which only some cards accept: they open
/// on any NVIDIA card, with them or without.
#[test]
fn nvenc_records_with_the_quality_presets() {
    let Ok((device, context)) = d3d::create_device(Some(0x10de)) else {
        vixeeny_common::skip_test("no Direct3D device");
        return;
    };
    if !is_nvidia(&device) {
        vixeeny_common::skip_test("no NVIDIA card");
        return;
    }
    for id in ["nvenc_h264", "nvenc_hevc", "nvenc_av1"] {
        let got = record_gpu_with(
            &device,
            &context,
            id,
            Feed::Direct,
            (8, Chroma::C420),
            PresetName::Quality,
        )
        .unwrap();
        assert_colour(got, id);
    }
}

#[test]
fn ten_bit_gpu_frames_are_unpacked_for_a_software_encoder() {
    let (device, context) = d3d::create_device(None).unwrap();
    if let Some(got) = record_gpu(
        &device,
        &context,
        "libx265",
        Feed::Download,
        10,
        Chroma::C420,
    ) {
        assert_colour(got, "x265 10-bit via the GPU");
    }
}

#[test]
fn full_and_half_chroma_come_from_the_gpu_for_a_software_encoder() {
    let (device, context) = d3d::create_device(None).unwrap();
    for (id, depth, chroma) in [
        ("libx264", 8, Chroma::C444),
        ("libx264", 8, Chroma::C422),
        ("libx265", 10, Chroma::C444),
        ("libx265", 10, Chroma::C422),
    ] {
        if let Some(got) = record_gpu(&device, &context, id, Feed::Download, depth, chroma) {
            assert_colour(
                got,
                &format!("{id} {depth}-bit {} via the GPU", chroma.name()),
            );
        }
    }
}

/// The levels of the HDR monitor the HDR tests record.
const LEVELS: vixeeny_encode::card_hdr::Levels = vixeeny_encode::card_hdr::Levels {
    sdr_white_nits: 240.0,
    peak_nits: 1000.0,
};

/// A solid scRGB texture, as an HDR monitor's capture frame is.
fn hdr_texture(device: &Device, rgb: [f32; 3]) -> ID3D11Texture2D {
    let [r, g, b] = rgb;
    let pixels: Vec<u16> = [r, g, b, 1.0]
        .map(vixeeny_common::half::from_f32)
        .repeat((W * H) as usize);
    let bytes: Vec<u8> = pixels.iter().flat_map(|v| v.to_le_bytes()).collect();
    let bind = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
    let format = windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;
    vixeeny_platform::d3d::texture(device, (W, H), format, bind, 0, Some((&bytes, W * 8))).unwrap()
}

/// Records 10 frames of a solid HDR colour through the GPU path of `id`, kept as HDR or tone
/// mapped; the centre of the first frame, on 8 bits. `None` (skipped) without a GPU video
/// processor.
fn record_hdr(
    device: &Device,
    context: &DeviceContext,
    (id, feed, depth): (&str, Feed, u8),
    keep_hdr: bool,
    rgb: [f32; 3],
) -> Option<[i32; 3]> {
    let registry = Registry::builtin().unwrap();
    let pipeline = match GpuPipeline::new(
        device,
        context,
        Source {
            size: (W, H),
            hdr: Some(LEVELS),
        },
        (W, H),
        Fps::whole(30),
        keep_hdr,
        depth == 10,
        Chroma::C420,
        feed,
    ) {
        Ok(p) => p,
        Err(e) => {
            vixeeny_common::skip_test(format!("no GPU video processor ({e})"));
            return None;
        }
    };
    let cfg = RecordConfig {
        depth,
        hdr: keep_hdr,
        ..common::config(&registry, id, PresetName::Performance)
    };
    // Tests run side by side: each recording has its own folder.
    static RUN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let run = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("hdr-{id}-{keep_hdr}-{run}");
    Some(record_texture(
        cfg,
        pipeline,
        &hdr_texture(device, rgb),
        &name,
    ))
}

/// An HDR monitor recorded on the GPU, whatever the video processor of the card takes: kept as
/// PQ (the codes the processor's coding gives), or tone mapped to SDR as the screenshots are.
#[test]
fn hdr_monitors_are_recorded_on_the_gpu_kept_or_tone_mapped() {
    let (device, context) = d3d::create_device(None).unwrap();
    // A 1000-nit orange kept as HDR: its BT.2020 / PQ Y, Cb, Cr.
    let orange = [10.0, 4.0, 0.2];
    let [r, g, b] = vixeeny_common::colour::scrgb_to_pq(orange).map(|c| f64::from(c) / 65535.0);
    let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
    let want = [
        64.0 + 876.0 * y,
        512.0 + 896.0 * (b - y) / 1.8814,
        512.0 + 896.0 * (r - y) / 1.4746,
    ]
    .map(|v| (v / 4.0).round() as i32);
    let Some(got) = record_hdr(
        &device,
        &context,
        ("libx265", Feed::Download, 10),
        true,
        orange,
    ) else {
        return;
    };
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() <= 2, "HDR: got {got:?}, want {want:?}");
    }
    // A grey at half SDR white made SDR: nothing brighter, so as an SDR capture shows it.
    let white = LEVELS.sdr_white_nits / 80.0;
    let got = record_hdr(
        &device,
        &context,
        ("libx264", Feed::Download, 8),
        false,
        [white * 0.5; 3],
    )
    .unwrap();
    let code = (vixeeny_common::colour::srgb_encode(0.5) * 255.0).round();
    let luma = (16.0 + 219.0 * code / 255.0).round() as i32;
    assert!(
        (got[0] - luma).abs() <= 2,
        "SDR: got {got:?}, want Y {luma}"
    );
    assert!(
        got[1..].iter().all(|c| (c - 128).abs() <= 2),
        "SDR: {got:?}"
    );
}

/// NVENC reads the HDR frames the shaders write as they are.
#[test]
fn nvenc_records_hdr_from_the_shaders() {
    let Ok((device, context)) = d3d::create_device(Some(0x10de)) else {
        vixeeny_common::skip_test("no Direct3D device");
        return;
    };
    let nvidia = {
        use windows::core::Interface;
        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast().unwrap();
        // SAFETY: plain queries of a live device.
        let adapter = unsafe { dxgi.GetAdapter() }.unwrap();
        // SAFETY: as above.
        unsafe { adapter.GetDesc() }.unwrap().VendorId == 0x10de
    };
    if !nvidia || vixeeny_encode::ffmpeg::encoder::find_by_name("hevc_nvenc").is_none() {
        vixeeny_common::skip_test("no NVENC");
        return;
    }
    let white = LEVELS.sdr_white_nits / 80.0;
    let kept = record_hdr(
        &device,
        &context,
        ("nvenc_hevc", Feed::Direct, 10),
        true,
        [white; 3],
    )
    .unwrap();
    let downloaded = record_hdr(
        &device,
        &context,
        ("libx265", Feed::Download, 10),
        true,
        [white; 3],
    )
    .unwrap();
    for (a, b) in kept.iter().zip(downloaded) {
        assert!((a - b).abs() <= 2, "NVENC {kept:?}, x265 {downloaded:?}");
    }
}
