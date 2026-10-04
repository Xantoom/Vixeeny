// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask verify-registry`: the registry against the real encoders of the linked FFmpeg
//! (plan 6.1), and the probe against the software encoders (CA-REC-6 for what runs everywhere).
//! Needs the native build and `--features ffmpeg-next`.
#![cfg(feature = "ffmpeg-next")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use vixeeny_encode::ffmpeg_probe::{FfmpegProber, verify};
use vixeeny_encode::probe::probe;
use vixeeny_encode::registry::{Chroma, Registry};

#[test]
fn the_registry_matches_ffmpeg() {
    let registry = Registry::builtin().unwrap();
    // The Windows FFmpeg build ships every hardware encoder (compiled in, GPU or not).
    let require_hardware = cfg!(windows);
    let mismatches = verify(&registry, require_hardware);
    for m in &mismatches {
        eprintln!("{}: {}", m.encoder, m.what);
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatch(es) between registry.toml and FFmpeg",
        mismatches.len()
    );
}

#[test]
fn the_probe_finds_the_software_encoders() {
    let registry = Registry::builtin().unwrap();
    let result = probe(&registry, &FfmpegProber::new(Vec::new()), "test");
    for (id, depth, chroma) in [
        ("libx264", 8, Chroma::C420),
        ("libx265", 8, Chroma::C420),
        ("libx265", 10, Chroma::C420),
        ("libsvtav1", 8, Chroma::C420),
        ("libsvtav1", 10, Chroma::C420),
        ("libvpx_vp9", 8, Chroma::C420),
    ] {
        assert!(
            result.supports(id, depth, chroma, false),
            "{id} {depth}-bit {chroma:?}"
        );
    }
    // HDR signalling opens with the 10-bit HEVC and AV1 software encoders.
    assert!(result.supports("libx265", 10, Chroma::C420, true));
    assert!(result.supports("libsvtav1", 10, Chroma::C420, true));
    for e in &result.encoders {
        eprintln!(
            "{} {:?}",
            e.id,
            e.formats
                .iter()
                .map(|f| (f.depth, f.chroma.name(), f.hdr))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_verifier_catches_wrong_data() {
    let text = include_str!("../codecs/registry.toml");
    let wrong_option =
        Registry::parse(&text.replacen("ffmpeg_option = \"crf\"", "ffmpeg_option = \"crfx\"", 1))
            .unwrap();
    let found = verify(&wrong_option, false);
    assert!(found.iter().any(|m| m.what.contains("crfx")), "{found:?}");
    let wrong_value =
        Registry::parse(&text.replacen("\"best\", \"good\"", "\"bestest\", \"good\"", 1)).unwrap();
    let found = verify(&wrong_value, false);
    assert!(
        found.iter().any(|m| m.what.contains("bestest")),
        "{found:?}"
    );
    let wrong_pixel =
        Registry::parse(&text.replacen("ffmpeg = \"yuv420p\"", "ffmpeg = \"ya8\"", 1)).unwrap();
    assert!(!verify(&wrong_pixel, false).is_empty());
}
