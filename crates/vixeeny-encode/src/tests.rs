// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::RefCell;
use std::path::PathBuf;

use vixeeny_common::config::Video;

use crate::probe::{
    Adapter, EncoderProbe, FormatProbe, ProbeResult, Prober, cache_key, cached_or_probe, from_toml,
    probe, to_toml, vendor_from_pci,
};
use crate::registry::{
    Chroma, Container, Encoder, Family, Kind, PixelFormatSpec, Registry, Vendor,
};
use crate::validate::{Context, IssueKind, Severity, output_size, pick_auto, validate};

fn registry() -> Registry {
    Registry::builtin().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_builtin_registry_covers_plan_6_2() {
    let r = registry();
    // Every encoder of the table of section 6.2 is there.
    for id in [
        "libx264",
        "libx265",
        "libsvtav1",
        "libvpx_vp9",
        "nvenc_h264",
        "nvenc_hevc",
        "nvenc_av1",
        "amf_h264",
        "amf_hevc",
        "amf_av1",
        "qsv_h264",
        "qsv_hevc",
        "qsv_av1",
        "qsv_vp9",
    ] {
        assert!(r.get(id).is_some(), "{id} missing");
    }
    assert_eq!(r.encoders.len(), 14);
}

#[test]
fn containers_follow_the_matrix_13_2() {
    for e in &registry().encoders {
        let webm = e.containers.contains(&Container::Webm);
        assert_eq!(
            webm,
            matches!(e.family, Family::Av1 | Family::Vp9),
            "{}",
            e.id
        );
        assert!(e.containers.contains(&Container::Mkv), "{}", e.id);
        assert!(e.containers.contains(&Container::Mp4), "{}", e.id);
    }
}

#[test]
fn hdr_is_hevc_and_av1_only_with_10_bits() {
    for e in &registry().encoders {
        if e.hdr {
            assert!(matches!(e.family, Family::Hevc | Family::Av1), "{}", e.id);
            assert!(e.pixel_formats.iter().any(|p| p.depth == 10), "{}", e.id);
        }
    }
}

#[test]
fn every_encoder_has_four_presets_and_sane_params() {
    for e in &registry().encoders {
        for name in [
            crate::registry::PresetName::Quality,
            crate::registry::PresetName::Balanced,
            crate::registry::PresetName::Performance,
            crate::registry::PresetName::Small,
        ] {
            assert!(!e.presets.get(name).is_empty(), "{} {name:?}", e.id);
        }
        let mut keys: Vec<_> = e.params.iter().map(|p| &p.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), e.params.len(), "{}: duplicate param keys", e.id);
    }
}

fn custom(id: &str, params: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    let params = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    registry().get(id).unwrap().custom_options(&params)
}

#[test]
fn the_custom_options_follow_the_rate_mode() {
    // Constant quality: the quality alone, the defaults of the other parameters.
    let o = custom("nvenc_h264", &[("rc.quality", "28")]);
    assert_eq!(o["rc"], "vbr");
    assert_eq!(o["cq"], "28");
    assert_eq!(o["preset"], "p5");
    assert!(!o.contains_key("b") && !o.contains_key("bf") && !o.contains_key("g"));
    // VBR: no quality left over, the rates in bit/s, two seconds of buffer.
    let o = custom(
        "nvenc_h264",
        &[
            ("rc.mode", "vbr"),
            ("rc.quality", "28"),
            ("rc.bitrate", "8000"),
            ("rc.maxrate", "12000"),
        ],
    );
    assert!(!o.contains_key("cq"));
    assert_eq!(
        (o["b"].as_str(), o["maxrate"].as_str()),
        ("8000000", "12000000")
    );
    assert_eq!(o["bufsize"], "24000000");
    // A constant bitrate (older settings) is the variable one now.
    let o = custom("libx264", &[("rc.mode", "cbr"), ("rc.bitrate", "6000")]);
    assert_eq!(o["b"], "6000000");
    assert!(!o.contains_key("crf"));
    // A maximum below the target is raised to it.
    let o = custom(
        "libx264",
        &[
            ("rc.mode", "vbr"),
            ("rc.bitrate", "9000"),
            ("rc.maxrate", "1000"),
        ],
    );
    assert_eq!(o["maxrate"], "9000000");
    // A mode the encoder does not know falls back to constant quality.
    let o = custom("libsvtav1", &[("rc.mode", "nope")]);
    assert_eq!(o["crf"], "30");
    // AMF quality sets every frame type; AV1 counts on 0-255.
    let o = custom("amf_av1", &[("rc.quality", "300")]);
    assert_eq!((o["rc"].as_str(), o["qp_b"].as_str()), ("cqp", "255"));
    // Values out of range are clamped, unknown ones fall back to the default, `auto` is left out.
    let o = custom(
        "libx264",
        &[("keyint", "120"), ("tune", "nope"), ("bframes", "99")],
    );
    assert_eq!(o["g"], "120");
    assert_eq!(o["bf"], "16");
    assert!(!o.contains_key("tune"));
    let o = custom("amf_h264", &[("bframes", "2")]);
    assert_eq!(o["bf"], "2");
}

#[test]
fn a_broken_registry_is_rejected() {
    let text = include_str!("../codecs/registry.toml");
    assert!(
        Registry::parse(&format!("{text}\n{text}")).is_err(),
        "duplicate ids"
    );
    assert!(Registry::parse(&text.replace("kind = \"hardware\"", "kind = \"software\"")).is_err());
    assert!(Registry::parse("[[encoder]]\nid = \"x\"").is_err());
}

// ---- validation ---------------------------------------------------------------------------

fn profile(encoder: &str, container: &str) -> Video {
    Video {
        encoder: encoder.into(),
        container: container.into(),
        ..Video::default()
    }
}

fn check(profile: &Video, probe: Option<&ProbeResult>) -> Vec<IssueKind> {
    let r = registry();
    validate(
        profile,
        &Context {
            registry: &r,
            source: (3840, 2160),
            probe,
        },
    )
    .into_iter()
    .map(|i| i.kind)
    .collect()
}

#[test]
fn the_default_profile_is_valid() {
    let r = registry();
    let issues = validate(
        &Video::default(),
        &Context {
            registry: &r,
            source: (1920, 1080),
            probe: None,
        },
    );
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn encoder_and_container_must_agree() {
    assert!(check(&profile("libx264", "mkv"), None).is_empty());
    let issues = check(&profile("libx264", "webm"), None);
    assert!(issues.contains(&IssueKind::ContainerNotSupported {
        family: Family::H264,
        container: Container::Webm
    }));
    assert!(check(&profile("libsvtav1", "webm"), None).is_empty());
    assert!(
        check(&profile("nope", "mkv"), None).contains(&IssueKind::UnknownEncoder("nope".into()))
    );
    assert!(
        check(&profile("libx264", "avi"), None)
            .contains(&IssueKind::UnknownContainer("avi".into()))
    );
}

#[test]
fn pixel_format_and_hdr_rules() {
    let mut p = profile("nvenc_h264", "mp4_hybrid");
    p.chroma = "422".into();
    assert!(check(&p, None).contains(&IssueKind::FormatNotSupported {
        depth: 8,
        chroma: Chroma::C422
    }));

    let mut hdr = profile("nvenc_hevc", "mp4_hybrid");
    hdr.hdr = "keep_hdr".into();
    assert!(check(&hdr, None).is_empty());
    hdr.container = "webm".into();
    assert!(check(&hdr, None).contains(&IssueKind::HdrContainer(Container::Webm)));
    let mut h264 = profile("libx264", "mkv");
    h264.hdr = "keep_hdr".into();
    assert!(check(&h264, None).contains(&IssueKind::HdrNeedsHevcOrAv1));
}

#[test]
fn audio_and_container() {
    let mut p = profile("libx264", "mp4_hybrid");
    p.audio.codec = "pcm".into();
    assert!(check(&p, None).contains(&IssueKind::AudioCodec {
        codec: "pcm".into(),
        container: Container::Mp4
    }));
    p.container = "mkv".into();
    assert!(check(&p, None).is_empty());
    p.audio.codec = "opus".into();
    p.container = "webm".into();
    p.encoder = "libsvtav1".into();
    assert!(check(&p, None).is_empty());
    p.audio.codec = "aac".into();
    assert!(!check(&p, None).is_empty());
    p.audio.codec = "wma".into();
    assert!(check(&p, None).contains(&IssueKind::UnknownAudioCodec("wma".into())));
}

#[test]
fn frame_rate_is_checked_against_the_codec_level() {
    let mut p = profile("libx264", "mkv");
    p.fps = 240;
    let r = registry();
    let issues = validate(
        &p,
        &Context {
            registry: &r,
            source: (3840, 2160),
            probe: None,
        },
    );
    let warning = issues
        .iter()
        .find(|i| matches!(i.kind, IssueKind::FramerateTooHigh { .. }));
    assert_eq!(warning.map(|i| i.severity), Some(Severity::Warning));
    assert!(
        issues.iter().all(|i| i.severity != Severity::Error),
        "a warning does not block"
    );
    assert!(check(&profile("libx265", "mkv"), None).is_empty());
    p.encoder = "libx265".into();
    assert!(check(&p, None).is_empty(), "HEVC level 6.2 handles 4K 240");
}

#[test]
fn bad_numbers_are_reported() {
    let p = Video {
        fps: 0,
        chroma: "411".into(),
        resolution: "huge".into(),
        preset: "ludicrous".into(),
        hdr: "maybe".into(),
        ..Video::default()
    };
    let issues = check(&p, None);
    for expected in [
        IssueKind::ZeroFps,
        IssueKind::UnknownChroma("411".into()),
        IssueKind::BadResolution("huge".into()),
        IssueKind::UnknownPreset("ludicrous".into()),
        IssueKind::UnknownHdrSetting("maybe".into()),
    ] {
        assert!(issues.contains(&expected), "{expected:?} in {issues:?}");
    }
}

#[test]
fn resolution_settings() {
    assert_eq!(output_size("source", (2560, 1440)), Some((2560, 1440)));
    assert_eq!(output_size("1080p", (2560, 1440)), Some((1920, 1080)));
    assert_eq!(output_size("720p", (3440, 1440)), Some((1720, 720)));
    assert_eq!(output_size("1280x720", (1, 1)), Some((1280, 720)));
    assert_eq!(output_size("0x720", (1, 1)), None);
    assert_eq!(output_size("p", (1, 1)), None);
}

/// The property of 6.4, over every encoder × container × depth × chroma × HDR: the verdict
/// agrees with the registry data written out by hand.
#[test]
fn validation_agrees_with_the_data_exhaustively() {
    let r = registry();
    let containers = ["mkv", "mp4_hybrid", "mp4_fragmented", "webm"];
    for e in r.encoders() {
        for container in containers {
            for chroma in ["420", "422", "444"] {
                for hdr in ["tonemap_sdr", "keep_hdr"] {
                    let p = Video {
                        encoder: e.id.clone(),
                        container: container.into(),
                        chroma: chroma.into(),
                        hdr: hdr.into(),
                        ..Video::default()
                    };
                    let c = Container::from_setting(container).unwrap_or_else(|| panic!());
                    let ch = Chroma::from_setting(chroma).unwrap_or_else(|| panic!());
                    let depth = crate::validate::depth(e, ch, hdr == "keep_hdr", None);
                    let want = e.containers.contains(&c)
                        && e.supports_format(depth, ch)
                        && (hdr == "tonemap_sdr" || (e.hdr && c != Container::Webm));
                    let issues = check(&p, None);
                    let got = !issues
                        .iter()
                        .any(|k| !matches!(k, IssueKind::FramerateTooHigh { .. }));
                    assert_eq!(
                        got, want,
                        "{} {container} {depth}-bit {chroma} {hdr}: {issues:?}",
                        e.id
                    );
                }
            }
        }
    }
}

// ---- probing ------------------------------------------------------------------------------

fn gpu(index: u32, name: &str, vendor_id: u32, driver: &str) -> Adapter {
    Adapter {
        index,
        name: name.into(),
        vendor: vendor_from_pci(vendor_id),
        vendor_id,
        device_id: 0x2684,
        driver_version: driver.into(),
        software: vendor_id == 0x1414,
    }
}

/// (encoder id, adapter index, size, hdr)
type Call = (String, Option<u32>, (u32, u32), bool);

/// Which (encoder, adapter) pairs open, and what they accept.
struct Fake {
    adapters: Vec<Adapter>,
    calls: RefCell<Vec<Call>>,
}

impl Prober for Fake {
    fn adapters(&self) -> Vec<Adapter> {
        self.adapters.clone()
    }

    fn try_open(
        &self,
        encoder: &Encoder,
        adapter: Option<&Adapter>,
        format: &PixelFormatSpec,
        size: (u32, u32),
        hdr: bool,
    ) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push((encoder.id.clone(), adapter.map(|a| a.index), size, hdr));
        // The second NVIDIA card is an old one: no HEVC 10-bit, nothing at 4K.
        if encoder.vendor == Vendor::Nvidia
            && adapter.is_some_and(|a| a.index == 1)
            && (encoder.family != Family::H264 || format.depth > 8 || size.0 > 256)
        {
            return Err("old card".into());
        }
        // The Intel iGPU has no HDR.
        if encoder.vendor == Vendor::Intel && hdr {
            return Err("no hdr".into());
        }
        // AV1 encoding needs a recent NVIDIA card (index 0 here).
        if encoder.id == "nvenc_av1" && adapter.is_some_and(|a| a.index != 0) {
            return Err("no av1".into());
        }
        // The software encoders accept everything except 4:4:4 AV1 (not declared anyway).
        Ok(())
    }
}

fn multi_gpu() -> Fake {
    Fake {
        adapters: vec![
            gpu(0, "NVIDIA GeForce RTX 5080", 0x10DE, "32.0.15.7000"),
            gpu(1, "NVIDIA GeForce GTX 960", 0x10DE, "32.0.15.7000"),
            gpu(0, "Intel(R) UHD Graphics", 0x8086, "31.0.101.5000"),
            gpu(0, "Microsoft Basic Render Driver", 0x1414, "10.0"),
        ],
        calls: RefCell::default(),
    }
}

fn find<'a>(r: &'a ProbeResult, id: &str, adapter: Option<u32>) -> Option<&'a EncoderProbe> {
    r.encoders
        .iter()
        .find(|e| e.id == id && e.adapter == adapter)
}

#[test]
fn probing_associates_each_encoder_with_its_gpu() {
    let fake = multi_gpu();
    let result = probe(&registry(), &fake, "0.1");

    // NVIDIA: both cards listed, with their own capabilities.
    let new = find(&result, "nvenc_hevc", Some(0)).unwrap_or_else(|| panic!("RTX missing"));
    assert!(
        new.formats
            .iter()
            .any(|f| f.depth == 10 && f.chroma == Chroma::C420 && f.uhd && f.hdr)
    );
    assert!(
        find(&result, "nvenc_hevc", Some(1)).is_none(),
        "old card cannot do HEVC"
    );
    let old = find(&result, "nvenc_h264", Some(1)).unwrap_or_else(|| panic!("GTX missing"));
    assert!(old.formats.iter().all(|f| !f.uhd));
    assert!(find(&result, "nvenc_av1", Some(1)).is_none());
    assert!(find(&result, "nvenc_av1", Some(0)).is_some());

    // Intel: only the iGPU, no HDR.
    let qsv = find(&result, "qsv_hevc", Some(0)).unwrap_or_else(|| panic!("iGPU missing"));
    assert!(qsv.formats.iter().all(|f| !f.hdr));
    // AMD: no AMD adapter, so no AMF entry at all.
    assert!(result.encoders.iter().all(|e| !e.id.starts_with("amf_")));
    // The software renderer is never used, and software encoders run once.
    assert!(
        fake.calls
            .borrow()
            .iter()
            .all(|(id, a, ..)| !(id.starts_with("nvenc") && *a == Some(2)))
    );
    assert_eq!(
        result.encoders.iter().filter(|e| e.id == "libx264").count(),
        1
    );
    assert!(find(&result, "libx264", None).is_some());
    // Hardware encoders are tried at 256×256 first; 4K only after that worked.
    let calls = fake.calls.borrow();
    let first = calls
        .iter()
        .position(|c| c.0 == "nvenc_h264" && c.1 == Some(1))
        .unwrap_or(0);
    assert_eq!(calls[first].2, (256, 256));
    // HDR is only attempted for encoders and formats declared for it.
    assert!(
        calls
            .iter()
            .filter(|c| c.3)
            .all(|c| c.0 != "nvenc_h264" && c.0 != "libx264")
    );
    // Vulkan and VAAPI do not exist on Windows.
    assert!(
        result
            .encoders
            .iter()
            .all(|e| !e.id.starts_with("vaapi") && !e.id.starts_with("vulkan"))
    );
}

#[test]
fn a_system_without_a_gpu_only_offers_software() {
    let fake = Fake {
        adapters: vec![],
        calls: RefCell::default(),
    };
    let result = probe(&registry(), &fake, "0.1");
    assert!(result.encoders.iter().all(|e| e.id.starts_with("lib")));
    assert!(result.supports("libx264", 8, Chroma::C420, false));
    let r = registry();
    assert_eq!(
        pick_auto(&r, Some(&result)).map(|e| e.id.as_str()),
        Some("libx264")
    );
    assert_eq!(pick_auto(&r, None).map(|e| e.id.as_str()), Some("libx264"));
}

#[test]
fn auto_prefers_av1_on_the_best_vendor_then_hevc_then_h264() {
    let r = registry();
    let result = probe(&r, &multi_gpu(), "0.1");
    assert_eq!(
        pick_auto(&r, Some(&result)).map(|e| e.id.as_str()),
        Some("nvenc_av1")
    );
    // Without AV1, HEVC wins; with only H.264, H.264.
    let mut only_hevc = result.clone();
    only_hevc
        .encoders
        .retain(|e| e.id.contains("hevc") || e.id.contains("h264") || e.id.starts_with("lib"));
    assert_eq!(
        pick_auto(&r, Some(&only_hevc)).map(|e| e.id.as_str()),
        Some("nvenc_hevc")
    );
    let mut only_hevc = result;
    only_hevc
        .encoders
        .retain(|e| e.id == "nvenc_hevc" || e.id.starts_with("lib"));
    assert_eq!(
        pick_auto(&r, Some(&only_hevc)).map(|e| e.id.as_str()),
        Some("nvenc_hevc")
    );
    let intel = ProbeResult {
        encoders: vec![EncoderProbe {
            id: "qsv_h264".into(),
            adapter: Some(0),
            formats: vec![FormatProbe {
                depth: 8,
                chroma: Chroma::C420,
                uhd: true,
                hdr: false,
            }],
        }],
        ..ProbeResult::default()
    };
    assert_eq!(
        pick_auto(&r, Some(&intel)).map(|e| e.id.as_str()),
        Some("qsv_h264")
    );
}

#[test]
fn availability_drives_validation() {
    let r = registry();
    let result = probe(&r, &multi_gpu(), "0.1");
    // AMF was not found: choosing it is reported.
    assert!(check(&profile("amf_h264", "mkv"), Some(&result)).contains(&IssueKind::NotAvailable));
    assert!(check(&profile("nvenc_h264", "mkv"), Some(&result)).is_empty());
    // The iGPU cannot do HDR: asking for it is "not available".
    let mut p = profile("qsv_hevc", "mkv");
    p.hdr = "keep_hdr".into();
    assert!(check(&p, Some(&result)).contains(&IssueKind::NotAvailable));
}

#[test]
fn results_round_trip_through_toml_and_the_cache_key_tracks_drivers() {
    let result = probe(&registry(), &multi_gpu(), "0.1");
    let text = to_toml(&result).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(from_toml(&text).unwrap_or_else(|e| panic!("{e}")), result);

    let a = multi_gpu().adapters;
    let key = cache_key(&a, "0.1");
    assert_eq!(
        key,
        cache_key(&a.iter().rev().cloned().collect::<Vec<_>>(), "0.1"),
        "order does not matter"
    );
    assert_ne!(key, cache_key(&a, "0.2"), "new probe identity");
    let mut newer = a;
    newer[0].driver_version = "32.0.15.8000".into();
    assert_ne!(key, cache_key(&newer, "0.1"), "new driver");
    newer.pop();
    assert_ne!(key, cache_key(&newer, "0.1"), "GPU removed");
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vixeeny-probe-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_cache_is_reused_until_the_key_changes() {
    let dir = scratch("cache");
    let path = dir.join("hw_cache.toml");
    let adapters = multi_gpu().adapters;
    let runs = RefCell::new(0);
    let run = || {
        *runs.borrow_mut() += 1;
        Ok(probe(&registry(), &multi_gpu(), "0.1"))
    };
    let first = cached_or_probe(&path, &adapters, "0.1", run).unwrap_or_else(|e| panic!("{e}"));
    let second = cached_or_probe(&path, &adapters, "0.1", run).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(*runs.borrow(), 1, "second call is served by the cache");
    assert_eq!(first, second);
    // A driver update invalidates it.
    let mut updated = adapters;
    updated[0].driver_version = "99".into();
    cached_or_probe(&path, &updated, "0.1", run).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(*runs.borrow(), 2);
    // A corrupt file is ignored.
    std::fs::write(&path, "garbage {{{").unwrap_or_else(|e| panic!("{e}"));
    cached_or_probe(&path, &updated, "0.1", run).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(*runs.borrow(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_probe_identity_does_not_follow_the_app_version() {
    let id = crate::probe::identity();
    assert_eq!(id, crate::probe::identity(), "stable");
    assert!(!id.contains(env!("CARGO_PKG_VERSION")), "{id}");
    assert!(id.contains("registry") && id.contains("avcodec"), "{id}");
}

#[test]
fn a_probe_that_hangs_or_crashes_is_contained() {
    use crate::probe::{ProbeError, run_child};
    let dir = scratch("child");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let script = |name: &str, body: &str| {
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap_or_else(|e| panic!("{e}"));
        path
    };
    let hang = script("hang", "ping -n 30 127.0.0.1 >nul");
    let started = std::time::Instant::now();
    assert!(matches!(
        run_child(&hang, std::time::Duration::from_millis(200)),
        Err(ProbeError::Timeout)
    ));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the child was killed"
    );
    let crash = script("crash", "exit /b 3");
    assert!(matches!(
        run_child(&crash, std::time::Duration::from_secs(10)),
        Err(ProbeError::Failed(_))
    ));
    let junk = script("junk", "echo not toml {{{");
    assert!(matches!(
        run_child(&junk, std::time::Duration::from_secs(10)),
        Err(ProbeError::Output(_))
    ));
    let fine = script("fine", "echo key = \"k\"");
    let ok = run_child(&fine, std::time::Duration::from_secs(10)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ok.key, "k");
    assert!(matches!(
        run_child(
            std::path::Path::new("C:\\nonexistent\\vixeeny.exe"),
            std::time::Duration::from_secs(1)
        ),
        Err(ProbeError::Spawn(_))
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn software_encoders_are_software() {
    for e in &registry().encoders {
        assert_eq!(
            e.kind == Kind::Software,
            e.id.starts_with("lib"),
            "{}",
            e.id
        );
    }
}

#[test]
fn variable_frame_rate_needs_matroska_or_webm() {
    let registry = Registry::builtin().unwrap();
    let ctx = Context {
        registry: &registry,
        source: (1920, 1080),
        probe: None,
    };
    for (container, ok) in [("mkv", true), ("webm", true), ("mp4_hybrid", false)] {
        let mut p = profile("libx264", container);
        p.vfr = true;
        let has = validate(&p, &ctx)
            .iter()
            .any(|i| matches!(i.kind, IssueKind::VfrContainer(_)));
        // WebM carries no H.264 (another issue), but the VFR rule itself must not fire.
        assert_eq!(!has, ok, "{container}");
    }
}

#[test]
fn hevc_and_av1_record_in_10_bits_when_they_can() {
    use crate::validate::depth;
    let r = registry();
    let get = |id: &str| r.get(id).unwrap();
    for id in ["libx265", "libsvtav1", "nvenc_hevc", "nvenc_av1"] {
        assert_eq!(depth(get(id), Chroma::C420, false, None), 10, "{id}");
    }
    for id in ["libx264", "nvenc_h264", "libvpx_vp9"] {
        assert_eq!(depth(get(id), Chroma::C420, false, None), 8, "{id}");
    }
    // A GPU without 10-bit: 8 bits; HDR always asks for 10.
    let probe = ProbeResult::default();
    assert_eq!(
        depth(get("nvenc_hevc"), Chroma::C420, false, Some(&probe)),
        8
    );
    assert_eq!(
        depth(get("nvenc_hevc"), Chroma::C420, true, Some(&probe)),
        10
    );
    assert_eq!(depth(get("libx265"), Chroma::C420, false, Some(&probe)), 10);
}

#[test]
fn a_wide_screen_is_cropped_in_its_middle() {
    use crate::validate::{aspect, center_crop};
    assert_eq!(aspect("16:9"), Some((16, 9)));
    assert_eq!(aspect("source"), None);
    // 21:9 to 16:9: the sides go.
    assert_eq!(
        center_crop((3440, 1440), aspect("16:9")),
        (440, 0, 2560, 1440)
    );
    // 16:10 to 16:9: the top and the bottom.
    assert_eq!(
        center_crop((1920, 1200), aspect("16:9")),
        (0, 60, 1920, 1080)
    );
    assert_eq!(
        center_crop((1920, 1080), aspect("16:9")),
        (0, 0, 1920, 1080)
    );
    assert_eq!(center_crop((1920, 1080), None), (0, 0, 1920, 1080));
}
