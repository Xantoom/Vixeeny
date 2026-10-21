// SPDX-License-Identifier: GPL-3.0-or-later
//! The GPU path end to end (Windows): a solid colour texture is converted by the D3D11 video
//! processor, handed to the encoder (downloaded for a software encoder, as it is for NVENC when
//! this machine has one) and the decoded file holds the BT.709 limited-range values of that
//! colour, like the CPU path.
//! Needs the native build: `cargo xtask build-native ffmpeg`.
#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vixeeny_encode::clock::Fps;
use vixeeny_encode::d3d_convert::{self as d3d, Device, DeviceContext, Rect};
use vixeeny_encode::ffmpeg::{codec, format, frame, media};
use vixeeny_encode::gpu::{Feed, GpuPipeline, Source};
use vixeeny_encode::recorder::{
    FrameFormat, OutputContainer, RecordConfig, Recorder, Split, VideoFrame,
};
use vixeeny_encode::registry::{Chroma, PresetName, Registry};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

const W: u32 = 320;
const H: u32 = 180;
const MS: i64 = 1_000_000;
/// Orange: B, G, R, A.
const COLOUR: [u8; 4] = [40, 160, 220, 255];
/// Its BT.709 limited-range Y, Cb, Cr.
const YCBCR: [i32; 3] = [157, 69, 159];

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vixeeny-gpu-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn texture(device: &Device) -> ID3D11Texture2D {
    let pixels: Vec<u8> = COLOUR.repeat((W * H) as usize);
    let desc = D3D11_TEXTURE2D_DESC {
        Width: W,
        Height: H,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        ..Default::default()
    };
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr().cast(),
        SysMemPitch: W * 4,
        SysMemSlicePitch: 0,
    };
    let mut out = None;
    // SAFETY: valid descriptions; `pixels` outlives the call.
    unsafe { device.CreateTexture2D(&raw const desc, Some(&raw const data), Some(&raw mut out)) }
        .unwrap();
    out.unwrap()
}

fn config(
    registry: &Registry,
    id: &str,
    depth: u8,
    chroma: Chroma,
    preset: PresetName,
) -> RecordConfig {
    let encoder = registry.get(id).unwrap().clone();
    let options = encoder
        .presets
        .get(preset)
        .iter()
        .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
        .collect();
    RecordConfig {
        crop: None,
        encoder,
        options,
        container: OutputContainer::Mkv,
        output_size: (W, H),
        fps: Fps::whole(30),
        depth,
        chroma,
        hdr: false,
        peak_nits: 1000.0,
        split: Split::Off,
        keyframe_seconds: 1.0,
        queue: 1_000,
        vfr: false,
        audio: Vec::new(),
        gpu: None,
        replay_seconds: None,
        replay_storage: vixeeny_encode::replay::Storage::Ram,
        files: true,
        buffers: Default::default(),
    }
}

fn namer(dir: &Path) -> Box<dyn FnMut(u32) -> PathBuf + Send> {
    let dir = dir.to_path_buf();
    Box::new(move |part| dir.join(format!("rec-{part}.mkv")))
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
            hdr: false,
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
    let pipeline = Arc::new(pipeline);
    let mut cfg = config(&registry, id, depth, chroma, preset);
    cfg.gpu = Some(Arc::clone(&pipeline));
    let dir = scratch(&format!("{id}-{depth}-{}-{preset:?}", chroma.name()));
    let rec = Recorder::start(cfg, namer(&dir)).unwrap();
    let source = texture(device);
    let rect = Rect {
        left: 0,
        top: 0,
        right: W as i32,
        bottom: H as i32,
    };
    for i in 0..10 {
        let frame = pipeline.convert(&source, rect).unwrap();
        assert!(rec.push_hw(i * 1_000 * MS / 30, frame));
    }
    let summary = rec.stop(10 * 1_000 * MS / 30).unwrap();
    let got = centre(&summary.files[0]);
    let _ = std::fs::remove_dir_all(dir);
    Some(got)
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
        config(
            &registry,
            "libx264",
            8,
            Chroma::C420,
            PresetName::Performance,
        ),
        namer(&dir),
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
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn nvenc_reads_the_gpu_frames_directly_when_present() {
    let Ok((device, context)) = d3d::create_device(Some(0x10DE)) else {
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
    // Machines without an NVIDIA GPU fall back to the default adapter: skip them.
    let nvidia = {
        use windows::core::Interface;
        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast().unwrap();
        // SAFETY: plain queries of a live device.
        let adapter = unsafe { dxgi.GetAdapter() }.unwrap();
        // SAFETY: as above.
        unsafe { adapter.GetDesc() }.unwrap().VendorId == 0x10DE
    };
    if !nvidia {
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
    let Ok((device, context)) = d3d::create_device(Some(0x10DE)) else {
        vixeeny_common::skip_test("no Direct3D device");
        return;
    };
    let nvidia = {
        use windows::core::Interface;
        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast().unwrap();
        // SAFETY: plain queries of a live device.
        let adapter = unsafe { dxgi.GetAdapter() }.unwrap();
        // SAFETY: as above.
        unsafe { adapter.GetDesc() }.unwrap().VendorId == 0x10DE
    };
    if !nvidia {
        vixeeny_common::skip_test("no NVIDIA card");
        return;
    }
    for id in ["nvenc_h264", "nvenc_hevc"] {
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
