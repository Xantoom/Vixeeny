// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::RefCell;
use std::path::PathBuf;

use vixeeny_common::config::Video;

use crate::probe::{
    Adapter, DriverCaps, EncoderProbe, FormatProbe, ProbeResult, Prober, cache_key,
    cached_or_probe, from_toml, probe, to_toml, vendor_from_pci,
};
use crate::registry::{Chroma, Container, Encoder, Family, Registry, Vendor};
use crate::validate::{Context, IssueKind, Severity, output_size, pick_auto, validate};

fn registry() -> Registry {
    Registry::builtin().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn containers_follow_the_codec_matrix() {
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
    // AMF's QVBR counts the other way (51 the best): the user's scale is mirrored into it, and
    // a value set on the former CQP scale (0-255) falls back to the default.
    let o = custom("amf_av1", &[("rc.quality", "20")]);
    assert_eq!(
        (o["rc"].as_str(), o["qvbr_quality_level"].as_str()),
        ("qvbr", "32")
    );
    assert_eq!(o["preencode"], "1");
    assert_eq!(o["high_motion_quality_boost_enable"], "1");
    let o = custom("amf_av1", &[("rc.quality", "120")]);
    assert_eq!(o["qvbr_quality_level"], "44");
    // Values out of range are clamped, unknown ones fall back to the default, `auto` is left out.
    let o = custom(
        "libx264",
        &[("keyint", "120"), ("tune", "nope"), ("bframes", "99")],
    );
    assert_eq!(o["g"], "120");
    assert!(!o.contains_key("tune"));
    // x264's B-frames and look-ahead are the preset's until they are set.
    assert!(!o.contains_key("bf") && !o.contains_key("rc-lookahead"));
    assert_eq!(custom("libx264", &[("bframes", "2")])["bf"], "2");
    let o = custom("amf_h264", &[("bframes", "2")]);
    assert_eq!(o["bf"], "2");
}

#[test]
fn the_aac_of_media_foundation_is_only_used_up_to_192_kbits() {
    use crate::audio::AudioCodec;
    assert_eq!(AudioCodec::Aac.encoder_names(192), ["aac_mf", "aac"]);
    assert_eq!(AudioCodec::Aac.encoder_names(256), ["aac"]);
    assert_eq!(AudioCodec::Opus.encoder_names(320), ["libopus"]);
}

#[test]
fn amf_quality_eases_the_right_way_from_4k() {
    let r = registry();
    let amf = r.get("amf_hevc").unwrap();
    let level = |h| {
        amf.preset_options(crate::registry::PresetName::Quality, (3840, h), 60, None)
            ["qvbr_quality_level"]
            .clone()
    };
    // 51 is QVBR's best: easing lowers the level.
    assert_eq!(level(1080), "51");
    assert_eq!(level(2160), "49");
}

#[test]
fn the_custom_preset_starts_from_the_simple_one() {
    let r = registry();
    for id in ["libx264", "nvenc_hevc", "amf_av1", "qsv_h264", "libsvtav1"] {
        let encoder = r.get(id).unwrap();
        for preset in [
            crate::registry::PresetName::Quality,
            crate::registry::PresetName::Small,
        ] {
            let custom = encoder.custom_options(&encoder.preset_params(preset));
            // Every option of the preset that a parameter or the quality mode can set comes
            // out the same.
            for (key, value) in encoder.presets.get(preset) {
                let settable = encoder.params.iter().any(|p| &p.ffmpeg_option == key)
                    || encoder
                        .rate_control
                        .modes
                        .get(&crate::registry::RateMode::Quality)
                        .is_some_and(|m| m.contains_key(key));
                if settable {
                    assert_eq!(
                        custom.get(key),
                        Some(&value.to_ffmpeg()),
                        "{id} {preset:?} {key}"
                    );
                }
            }
        }
    }
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
    // Never larger than the screen.
    assert_eq!(output_size("2160p", (2560, 1440)), Some((2560, 1440)));
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

/// What the drivers describe; with `old_drivers` they cannot be asked.
struct Fake {
    adapters: Vec<Adapter>,
    old_drivers: bool,
}

impl Prober for Fake {
    fn adapters(&self) -> Vec<Adapter> {
        self.adapters.clone()
    }

    fn built_in(&self, _encoder: &Encoder) -> bool {
        true
    }

    fn driver_caps(&self, adapter: &Adapter) -> Result<Vec<DriverCaps>, String> {
        if self.old_drivers {
            return Err("too old".into());
        }
        let caps = |family, ten_bit, chroma: &[Chroma], max| DriverCaps {
            family,
            ten_bit,
            chroma: chroma.to_vec(),
            max,
            tools: None,
        };
        let all = [Chroma::C420, Chroma::C444];
        Ok(match (adapter.vendor, adapter.index) {
            (Vendor::Nvidia, 0) => vec![
                caps(Family::H264, false, &all, (8192, 8192)),
                caps(Family::Hevc, true, &all, (8192, 8192)),
                caps(Family::Av1, true, &[Chroma::C420], (8192, 8192)),
            ],
            // The old card: H.264 8-bit only, nothing at 4K.
            (Vendor::Nvidia, _) => vec![caps(Family::H264, false, &all, (2048, 2048))],
            // The iGPU: no 10-bit, so no HDR.
            (Vendor::Intel, _) => [Family::H264, Family::Hevc, Family::Av1, Family::Vp9]
                .into_iter()
                .map(|f| caps(f, false, &[Chroma::C420], (4096, 4096)))
                .collect(),
            _ => Vec::new(),
        })
    }
}

fn multi_gpu() -> Fake {
    Fake {
        adapters: vec![
            gpu(0, "NVIDIA GeForce RTX 5080", 0x10de, "32.0.15.7000"),
            gpu(1, "NVIDIA GeForce GTX 960", 0x10de, "32.0.15.7000"),
            gpu(0, "Intel(R) UHD Graphics", 0x8086, "31.001.5000"),
            gpu(0, "Microsoft Basic Render Driver", 0x1414, "10.0"),
        ],
        old_drivers: false,
    }
}

fn find<'a>(r: &'a ProbeResult, id: &str, adapter: Option<u32>) -> Option<&'a EncoderProbe> {
    r.encoders
        .iter()
        .find(|e| e.id == id && e.adapter == adapter)
}

/// What both ways of probing must find on `multi_gpu`.
fn check_multi_gpu(result: &ProbeResult) {
    // NVIDIA: both cards listed, with their own capabilities.
    let new = find(result, "nvenc_hevc", Some(0)).unwrap_or_else(|| panic!("RTX missing"));
    assert!(
        new.formats
            .iter()
            .any(|f| f.depth == 10 && f.chroma == Chroma::C420 && f.uhd && f.hdr)
    );
    assert!(
        find(result, "nvenc_hevc", Some(1)).is_none(),
        "old card cannot do HEVC"
    );
    let old = find(result, "nvenc_h264", Some(1)).unwrap_or_else(|| panic!("GTX missing"));
    assert!(old.formats.iter().all(|f| !f.uhd));
    assert!(find(result, "nvenc_av1", Some(1)).is_none());
    assert!(find(result, "nvenc_av1", Some(0)).is_some());

    // Intel: only the iGPU, no HDR.
    let qsv = find(result, "qsv_hevc", Some(0)).unwrap_or_else(|| panic!("iGPU missing"));
    assert!(qsv.formats.iter().all(|f| !f.hdr));
    // AMD: no AMD adapter, so no AMF entry at all.
    assert!(result.encoders.iter().all(|e| !e.id.starts_with("amf_")));
    assert_eq!(
        result.encoders.iter().filter(|e| e.id == "libx264").count(),
        1
    );
    assert!(find(result, "libx264", None).is_some());
    // Vulkan and VAAPI do not exist on Windows.
    assert!(
        result
            .encoders
            .iter()
            .all(|e| !e.id.starts_with("vaapi") && !e.id.starts_with("vulkan"))
    );
}

#[test]
fn probing_asks_the_drivers() {
    check_multi_gpu(&probe(&registry(), &multi_gpu(), "0.1"));
}

#[test]
fn a_driver_that_cannot_be_asked_offers_no_hardware_encoder() {
    let fake = Fake {
        old_drivers: true,
        ..multi_gpu()
    };
    let result = probe(&registry(), &fake, "0.1");
    assert!(result.encoders.iter().all(|e| e.adapter.is_none()));
    // Software encoders offer what the registry declares.
    assert!(result.supports("libx265", 10, Chroma::C444, true));
    assert!(
        !result.supports("libx264", 10, Chroma::C420, true),
        "no HDR for x264"
    );
}

#[test]
fn a_system_without_a_gpu_only_offers_software() {
    let fake = Fake {
        adapters: vec![],
        old_drivers: false,
    };
    let result = probe(&registry(), &fake, "0.1");
    assert!(result.encoders.iter().all(|e| e.id.starts_with("lib")));
    assert!(result.supports("libx264", 8, Chroma::C420, false));
    let r = registry();
    assert_eq!(
        pick_auto(&r, Some(&result), None).map(|e| e.id.as_str()),
        Some("libx264")
    );
    assert_eq!(
        pick_auto(&r, None, None).map(|e| e.id.as_str()),
        Some("libx264")
    );
}

#[test]
fn auto_prefers_av1_on_the_best_vendor_then_hevc_then_h264() {
    let r = registry();
    let result = probe(&r, &multi_gpu(), "0.1");
    assert_eq!(
        pick_auto(&r, Some(&result), None).map(|e| e.id.as_str()),
        Some("nvenc_av1")
    );
    // Without AV1, HEVC wins; with only H.264, H.264.
    let mut only_hevc = result.clone();
    only_hevc
        .encoders
        .retain(|e| e.id.contains("hevc") || e.id.contains("h264") || e.id.starts_with("lib"));
    assert_eq!(
        pick_auto(&r, Some(&only_hevc), None).map(|e| e.id.as_str()),
        Some("nvenc_hevc")
    );
    let mut only_hevc = result;
    only_hevc
        .encoders
        .retain(|e| e.id == "nvenc_hevc" || e.id.starts_with("lib"));
    assert_eq!(
        pick_auto(&r, Some(&only_hevc), None).map(|e| e.id.as_str()),
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
            tools: None,
            max: None,
        }],
        ..ProbeResult::default()
    };
    assert_eq!(
        pick_auto(&r, Some(&intel), None).map(|e| e.id.as_str()),
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

#[test]
fn simple_presets_follow_the_resolution_and_frame_rate() {
    use crate::registry::PresetName;
    let r = registry();
    let preset = |id: &str, (w, h): (u64, u64), fps: u64| {
        r.get(id).unwrap().speed_for(w * h * fps, None).unwrap().1
    };
    // Without the generation, Turing's speeds (AV1: Ada's): P5 up to 1440p60, faster above,
    // the fastest where none keeps up.
    assert_eq!(preset("nvenc_hevc", (1920, 1080), 60), "p5");
    assert_eq!(preset("nvenc_hevc", (2560, 1440), 60), "p5");
    assert_eq!(preset("nvenc_hevc", (3840, 2160), 60), "p3");
    assert_eq!(preset("nvenc_hevc", (3840, 2160), 120), "p3");
    assert_eq!(preset("nvenc_av1", (2560, 1440), 120), "p5");
    assert_eq!(preset("nvenc_av1", (3840, 2160), 60), "p3");
    assert_eq!(preset("nvenc_av1", (3840, 2160), 120), "p1");
    // H.264 starts at P4, as good as P5 there.
    assert_eq!(preset("nvenc_h264", (2560, 1440), 60), "p4");
    assert_eq!(preset("nvenc_h264", (3840, 2160), 60), "p4");
    assert_eq!(preset("amf_av1", (3840, 2160), 60), "quality");
    assert_eq!(preset("amf_av1", (3840, 2160), 120), "balanced");
    assert_eq!(preset("libx264", (1920, 1080), 60), "veryfast");
    assert_eq!(preset("libx264", (3840, 2160), 60), "ultrafast");
    assert!(r.encoders().all(|e| e.speed_for(1, None).is_some()));
    // The registry's presets, at that speed; from 4K on, the quality scale eases two steps.
    let av1 = r.get("nvenc_av1").unwrap();
    let opts = |size, fps| av1.preset_options(PresetName::Quality, size, fps, None);
    assert_eq!(opts((1920, 1080), 60)["cq"], "24");
    assert_eq!(opts((3840, 2160), 60)["cq"], "26");
    assert_eq!(opts((3840, 2160), 120)["preset"], "p1");
    // The fast preset keeps its own speed.
    assert_eq!(
        av1.preset_options(PresetName::Performance, (1920, 1080), 60, None)["preset"],
        "p2"
    );
}

#[test]
fn nvenc_options_follow_what_the_card_has() {
    use crate::probe::Tools;
    use crate::registry::PresetName;
    use std::collections::BTreeMap;
    let r = registry();
    let fit = |id: &str, preset: PresetName, tools: Option<Tools>, choose: bool| {
        let e = r.get(id).unwrap();
        let mut o = e.preset_options(preset, (1920, 1080), 60, tools);
        e.fit_tools(&mut o, tools, choose, 1920 * 1080 * 60);
        o
    };
    let ada = Tools {
        max_bframes: 4,
        bframe_ref_middle: true,
        lookahead: true,
        temporal_aq: true,
        engine_fps: 0,
        amf_throughput: [0; 4],
        engines: 0,
        lookahead_level: 0,
        generation: Default::default(),
    };
    // Everything there: the look-ahead, which places up to 4 B-frames, used as references.
    let o = fit("nvenc_av1", PresetName::Small, Some(ada), true);
    assert_eq!(o["rc-lookahead"], "8");
    assert_eq!(o["bf"], "4");
    assert_eq!(o["b_ref_mode"], "middle");
    assert_eq!(o["temporal-aq"], "1");
    // H.264 never takes B-frames as references.
    let o = fit("nvenc_h264", PresetName::Quality, Some(ada), true);
    assert!(!o.contains_key("b_ref_mode"));
    assert_eq!(o["bf"], "4");
    // HEVC on Pascal: no B-frames, so none as references; no temporal AQ where it lacks.
    let pascal = Tools {
        max_bframes: 0,
        bframe_ref_middle: false,
        lookahead: true,
        temporal_aq: false,
        engine_fps: 0,
        amf_throughput: [0; 4],
        engines: 0,
        lookahead_level: 0,
        generation: Default::default(),
    };
    let o = fit("nvenc_hevc", PresetName::Quality, Some(pascal), true);
    assert_eq!(o["bf"], "0");
    assert!(!o.contains_key("b_ref_mode") && !o.contains_key("temporal-aq"));
    // Without the look-ahead: 2 B-frames.
    let o = fit(
        "nvenc_hevc",
        PresetName::Quality,
        Some(Tools {
            lookahead: false,
            ..ada
        }),
        true,
    );
    assert_eq!(o["bf"], "2");
    assert!(!o.contains_key("rc-lookahead"));
    // The fast preset keeps the encoder's own choices; an undescribed card is left alone.
    let o = fit("nvenc_hevc", PresetName::Performance, Some(ada), false);
    assert!(!o.contains_key("bf") && !o.contains_key("rc-lookahead"));
    let before =
        r.get("nvenc_hevc")
            .unwrap()
            .preset_options(PresetName::Quality, (1920, 1080), 60, None);
    let mut fitted = fit("nvenc_hevc", PresetName::Quality, None, true);
    // Constant quality gets a ceiling all the same: half a bit per pixel.
    assert_eq!(fitted.remove("maxrate").as_deref(), Some("62208000"));
    assert_eq!(fitted, before);
    // A count set by hand stays, within what the card takes.
    let e = r.get("nvenc_hevc").unwrap();
    let mut o = e.custom_options(&BTreeMap::from([("bframes".into(), "4".into())]));
    e.fit_tools(
        &mut o,
        Some(Tools {
            max_bframes: 2,
            ..ada
        }),
        true,
        1920 * 1080 * 60,
    );
    assert_eq!(o["bf"], "2");
}

#[test]
fn nvenc_analyses_deeper_when_it_has_the_time_and_splits_frames_last() {
    use crate::probe::{NvencGeneration, Tools};
    use crate::registry::PresetName;
    let r = registry();
    let blackwell = Tools {
        max_bframes: 4,
        bframe_ref_middle: true,
        lookahead: true,
        temporal_aq: true,
        engine_fps: 0,
        amf_throughput: [0; 4],
        engines: 2,
        lookahead_level: 3,
        generation: NvencGeneration::Blackwell,
    };
    let fit = |id: &str, preset, tools: Tools, (w, h): (u32, u32), fps: u32| {
        let e = r.get(id).unwrap();
        let mut o = e.preset_options(preset, (w, h), fps, Some(tools));
        e.fit_tools(
            &mut o,
            Some(tools),
            preset != PresetName::Performance,
            u64::from(w * h * fps),
        );
        o
    };
    // 1080p60: the deepest look-ahead, no split.
    let o = fit("nvenc_hevc", PresetName::Small, blackwell, (1920, 1080), 60);
    assert_eq!(o["lookahead_level"], "3");
    assert_eq!(o["split_encode_mode"], "disabled");
    assert_eq!(o["maxrate"], "62208000");
    // 4K60: HEVC still has the time, AV1 no longer.
    let o = fit("nvenc_hevc", PresetName::Small, blackwell, (3840, 2160), 60);
    assert_eq!(
        (o["preset"].as_str(), o["lookahead_level"].as_str()),
        ("p5", "3")
    );
    let o = fit("nvenc_av1", PresetName::Small, blackwell, (3840, 2160), 60);
    assert_eq!(o["preset"], "p5");
    assert!(!o.contains_key("lookahead_level"));
    // 4K120: HEVC at P3 keeps up alone; AV1 does not even at P1, so its frames are split,
    // which P3 then keeps up with.
    let o = fit(
        "nvenc_hevc",
        PresetName::Small,
        blackwell,
        (3840, 2160),
        120,
    );
    assert_eq!(
        (o["preset"].as_str(), o["split_encode_mode"].as_str()),
        ("p3", "disabled")
    );
    let o = fit("nvenc_av1", PresetName::Small, blackwell, (3840, 2160), 120);
    assert_eq!(
        (o["preset"].as_str(), o["split_encode_mode"].as_str()),
        ("p3", "forced")
    );
    assert!(!o.contains_key("lookahead_level"));
    // 8K60: split at the fastest preset.
    let o = fit("nvenc_hevc", PresetName::Small, blackwell, (7680, 4320), 60);
    assert_eq!(
        (o["preset"].as_str(), o["split_encode_mode"].as_str()),
        ("p3", "forced")
    );
    assert!(!o.contains_key("lookahead_level"));
    // One encoder: nothing to split; no level 3 on the card or in the fast preset: none.
    let single = Tools {
        engines: 1,
        lookahead_level: 0,
        ..blackwell
    };
    let o = fit("nvenc_hevc", PresetName::Small, single, (1920, 1080), 60);
    assert!(!o.contains_key("split_encode_mode") && !o.contains_key("lookahead_level"));
    let o = fit(
        "nvenc_hevc",
        PresetName::Performance,
        blackwell,
        (1920, 1080),
        60,
    );
    assert!(!o.contains_key("lookahead_level"));
    // AMF: a ceiling too; its pre-analysis only in 8 bits. Intel's ICQ has none.
    let amf = r.get("amf_hevc").unwrap();
    let mut o = amf.custom_options(&[("preanalysis".to_owned(), "1".to_owned())].into());
    amf.fit_tools(&mut o, None, true, 1920 * 1080 * 60);
    assert_eq!(o["maxrate"], "62208000");
    amf.fit_depth(&mut o, 8);
    assert_eq!(o["preanalysis"], "1");
    amf.fit_depth(&mut o, 10);
    assert!(!o.contains_key("preanalysis"));
    let qsv = r.get("qsv_hevc").unwrap();
    let mut o = qsv.preset_options(PresetName::Small, (1920, 1080), 60, None);
    qsv.fit_tools(&mut o, None, true, 1920 * 1080 * 60);
    assert!(!o.contains_key("maxrate"));
}

#[test]
fn amd_and_intel_options_follow_their_media_engine() {
    use crate::probe::{NOT_DESCRIBED, Tools};
    use crate::registry::PresetName;
    let r = registry();
    let rate = |(w, h): (u64, u64), fps: u64| w * h * fps;
    // RDNA 3 and later (194 FPS of 1080p at the balanced preset), with AV1's B-frames.
    let rdna = Tools {
        max_bframes: 3,
        bframe_ref_middle: false,
        lookahead: true,
        temporal_aq: false,
        engine_fps: 194,
        amf_throughput: [0; 4],
        engines: 0,
        lookahead_level: 0,
        generation: Default::default(),
    };
    let av1 = r.get("amf_av1").unwrap();
    let speed = |tools, size, fps| av1.speed_for(rate(size, fps), Some(tools)).unwrap().1;
    assert_eq!(speed(rdna, (1920, 1080), 60), "quality");
    assert_eq!(speed(rdna, (2560, 1440), 60), "balanced");
    assert_eq!(speed(rdna, (3840, 2160), 60), "speed");
    // B-frames only with room to spare beside the quality preset; never the pre-analysis.
    let fit = |tools: Tools, (w, h): (u32, u32), fps: u32| {
        let mut o = av1.preset_options(PresetName::Small, (w, h), fps, Some(tools));
        av1.fit_tools(
            &mut o,
            Some(tools),
            true,
            rate((w.into(), h.into()), fps.into()),
        );
        o
    };
    assert_eq!(fit(rdna, (1920, 1080), 30)["bf"], "2");
    assert!(!fit(rdna, (1920, 1080), 60).contains_key("bf"));
    let no_bframes = Tools {
        max_bframes: 0,
        ..rdna
    };
    assert!(!fit(no_bframes, (1920, 1080), 30).contains_key("bf"));
    assert!(!fit(rdna, (1920, 1080), 30).contains_key("preanalysis"));
    // The driver's own throughput, when it gives it, decides instead (macroblocks a second:
    // 8160 a 1080p frame).
    let said = Tools {
        amf_throughput: [8160 * 400, 8160 * 200, 8160 * 100, 8160 * 70],
        engines: 0,
        lookahead_level: 0,
        generation: Default::default(),
        ..rdna
    };
    assert_eq!(speed(said, (1920, 1080), 60), "quality");
    assert_eq!(speed(said, (2560, 1440), 60), "balanced");
    assert_eq!(speed(said, (3840, 2160), 60), "speed");
    assert_eq!(fit(said, (1920, 1080), 30)["bf"], "2");
    assert!(!fit(said, (1920, 1080), 60).contains_key("bf"));
    // Intel: the medium preset as far as the engine keeps up, Arc's or an older one's.
    let qsv = r.get("qsv_hevc").unwrap();
    let intel = |engine_fps, size, fps| {
        let tools = Tools {
            max_bframes: NOT_DESCRIBED,
            bframe_ref_middle: false,
            lookahead: false,
            temporal_aq: false,
            engine_fps,
            amf_throughput: [0; 4],
            engines: 0,
            lookahead_level: 0,
            generation: Default::default(),
        };
        qsv.speed_for(rate(size, fps), Some(tools)).unwrap().1
    };
    assert_eq!(intel(277, (2560, 1440), 60), "medium");
    assert_eq!(intel(277, (3840, 2160), 60), "fast");
    assert_eq!(intel(277, (3840, 2160), 120), "veryfast");
    assert_eq!(intel(80, (1920, 1080), 60), "medium");
    assert_eq!(intel(80, (2560, 1440), 60), "veryfast");
}

#[test]
fn pictures_larger_than_an_encoder_takes_go_to_one_that_takes_them() {
    let r = registry();
    let result = probe(&r, &multi_gpu(), "0.1");
    let ultrawide = (5120, 1440);
    // The RTX 5080 takes 8192 wide in every codec; the iGPU 4096.
    assert!(result.fits("nvenc_h264", ultrawide));
    assert!(!result.fits("qsv_av1", ultrawide));
    assert!(result.fits("qsv_av1", (3840, 2160)));
    let mut intel = result.clone();
    intel.encoders.retain(|e| !e.id.starts_with("nvenc"));
    let auto = |size| pick_auto(&r, Some(&intel), size).map(|e| e.id.clone());
    assert_eq!(auto(Some((3840, 2160))).as_deref(), Some("qsv_av1"));
    assert_eq!(auto(Some(ultrawide)).as_deref(), Some("libx264"));
    // Chosen by hand, it is reported.
    let issues = validate(
        &profile("qsv_hevc", "mkv"),
        &Context {
            registry: &r,
            source: ultrawide,
            probe: Some(&intel),
        },
    );
    assert!(
        issues
            .iter()
            .any(|i| i.kind == IssueKind::TooLarge { max: (4096, 4096) }
                && i.severity == Severity::Error)
    );
}
