// SPDX-License-Identifier: GPL-3.0-or-later
//! Validation of a recording profile against the registry (plan 6.4) and the choice of the
//! `auto` encoder. Pure functions: no FFmpeg, no OS.

use vixeeny_common::config::Video;

use crate::probe::ProbeResult;
use crate::registry::{Chroma, Container, Encoder, Family, Kind, PresetName, Registry, Vendor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The profile cannot work.
    Error,
    /// It works, with a drawback.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    UnknownEncoder(String),
    /// The probe did not find this encoder / format on any GPU.
    NotAvailable,
    UnknownContainer(String),
    ContainerNotSupported {
        family: Family,
        container: Container,
    },
    UnknownChroma(String),
    FormatNotSupported {
        depth: u8,
        chroma: Chroma,
    },
    UnknownHdrSetting(String),
    HdrNeedsHevcOrAv1,
    HdrContainer(Container),
    HdrNotSupported,
    /// Variable frame rate needs Matroska or WebM.
    VfrContainer(Container),
    AudioCodec {
        codec: String,
        container: Container,
    },
    UnknownAudioCodec(String),
    UnknownPreset(String),
    ZeroFps,
    BadResolution(String),
    /// More pixels per second than the codec's top level allows.
    FramerateTooHigh {
        max_fps: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub kind: IssueKind,
}

fn error(kind: IssueKind) -> Issue {
    Issue {
        severity: Severity::Error,
        kind,
    }
}

fn warning(kind: IssueKind) -> Issue {
    Issue {
        severity: Severity::Warning,
        kind,
    }
}

/// Everything besides the profile itself that the verdict depends on.
pub struct Context<'a> {
    pub registry: &'a Registry,
    /// Size of the captured source, for `source` resolution and the frame-rate limit.
    pub source: (u32, u32),
    /// `None` = no probe yet: availability is not checked.
    pub probe: Option<&'a ProbeResult>,
}

/// Highest luma sample rate of the top level of each codec (samples per second).
const fn max_sample_rate(family: Family) -> u64 {
    match family {
        Family::H264 => 530_841_600,                 // level 5.2
        Family::Hevc | Family::Av1 => 4_278_190_080, // level 6.2 / 6.3
        Family::Vp9 => 4_278_190_080,                // level 6.2
    }
}

/// Output size of a `resolution` setting: `source`, `2160p`…`720p` (the ratio is kept, never
/// larger than the source: 2160p of a 1440p screen is 1440p), or `WxH`.
pub fn output_size(setting: &str, source: (u32, u32)) -> Option<(u32, u32)> {
    if setting == "source" {
        return Some(source);
    }
    if let Some(lines) = setting.strip_suffix('p') {
        let h: u32 = lines.parse().ok().filter(|h| *h > 0)?;
        if h >= source.1 {
            return Some(source);
        }
        // To the nearest even width (854 × 480 for 16:9).
        let w = (u64::from(source.0) * u64::from(h) + u64::from(source.1.max(1)))
            / (2 * u64::from(source.1.max(1)))
            * 2;
        return Some(((w as u32).max(2), h & !1));
    }
    let (w, h) = setting.split_once('x')?;
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// The bit depth a recording gets: 10 bits for HDR, and for HEVC and AV1 whenever the encoder
/// (and, for a hardware one, the GPU) can (finer gradients, no banding, and a few percent
/// smaller at the same quality; every HEVC and AV1 decoder of the last years plays it). H.264
/// and VP9 stay 8-bit: their 10-bit profiles play almost nowhere in hardware.
pub fn depth(encoder: &Encoder, chroma: Chroma, hdr: bool, probe: Option<&ProbeResult>) -> u8 {
    if hdr {
        return 10;
    }
    let can = matches!(encoder.family, Family::Hevc | Family::Av1)
        && encoder.supports_format(10, chroma)
        && (encoder.kind == Kind::Software
            || probe.is_none_or(|p| p.supports(&encoder.id, 10, chroma, false)));
    if can { 10 } else { 8 }
}

/// An `aspect` setting: `source` (`None`) or `W:H`.
pub fn aspect(setting: &str) -> Option<(u32, u32)> {
    let (w, h) = setting.split_once(':')?;
    let (w, h): (u32, u32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// The middle part of a `size` picture with the `aspect` ratio (the whole picture without
/// one): `(left, top, width, height)`, even sizes.
pub fn center_crop(size: (u32, u32), aspect: Option<(u32, u32)>) -> (u32, u32, u32, u32) {
    let (w, h) = size;
    let Some((aw, ah)) = aspect else {
        return (0, 0, w, h);
    };
    let (aw, ah) = (u64::from(aw), u64::from(ah));
    // Wider than the ratio: the sides go; taller: the top and the bottom.
    let (cw, ch) = if u64::from(w) * ah > u64::from(h) * aw {
        (((u64::from(h) * aw / ah) as u32).min(w), h)
    } else {
        (w, ((u64::from(w) * ah / aw) as u32).min(h))
    };
    let (cw, ch) = (cw.max(2) & !1, ch.max(2) & !1);
    (
        (w - cw.min(w)) / 2,
        (h - ch.min(h)) / 2,
        cw.min(w),
        ch.min(h),
    )
}

/// Whether an audio codec fits a container (annex 13.2); `None` for an unknown codec.
fn audio_ok(codec: &str, container: Container) -> Option<bool> {
    Some(match codec {
        "aac" | "flac" => !matches!(container, Container::Webm),
        "opus" => true,
        "pcm" | "pcm16" | "pcm24" => matches!(container, Container::Mkv),
        _ => return None,
    })
}

pub fn validate(profile: &Video, ctx: &Context<'_>) -> Vec<Issue> {
    let mut issues = Vec::new();

    let container = Container::from_setting(&profile.container);
    if container.is_none() {
        issues.push(error(IssueKind::UnknownContainer(
            profile.container.clone(),
        )));
    }
    let chroma = Chroma::from_setting(&profile.chroma);
    if chroma.is_none() {
        issues.push(error(IssueKind::UnknownChroma(profile.chroma.clone())));
    }
    if profile.fps == 0 {
        issues.push(error(IssueKind::ZeroFps));
    }
    if PresetName::from_setting(&profile.preset).is_none() {
        issues.push(error(IssueKind::UnknownPreset(profile.preset.clone())));
    }
    let wants_hdr = match profile.hdr.as_str() {
        "tonemap_sdr" => false,
        "keep_hdr" | "hdr" => true,
        other => {
            issues.push(error(IssueKind::UnknownHdrSetting(other.to_owned())));
            false
        }
    };
    let size = output_size(&profile.resolution, ctx.source);
    if size.is_none() {
        issues.push(error(IssueKind::BadResolution(profile.resolution.clone())));
    }

    // Audio ↔ container.
    if profile.audio.codec != "auto" {
        match (
            audio_ok(&profile.audio.codec, container.unwrap_or(Container::Mkv)),
            container,
        ) {
            (None, _) => issues.push(error(IssueKind::UnknownAudioCodec(
                profile.audio.codec.clone(),
            ))),
            (Some(false), Some(c)) => issues.push(error(IssueKind::AudioCodec {
                codec: profile.audio.codec.clone(),
                container: c,
            })),
            _ => {}
        }
    }

    // HDR ↔ container (the encoder part is below).
    if profile.vfr
        && let Some(c) = container
        && !matches!(c, Container::Mkv | Container::Webm)
    {
        issues.push(error(IssueKind::VfrContainer(c)));
    }
    if wants_hdr
        && let Some(c) = container
        && c == Container::Webm
    {
        issues.push(error(IssueKind::HdrContainer(c)));
    }

    if profile.encoder == "auto" {
        return issues;
    }
    let Some(encoder) = ctx.registry.get(&profile.encoder) else {
        issues.push(error(IssueKind::UnknownEncoder(profile.encoder.clone())));
        return issues;
    };
    if let Some(c) = container
        && !encoder.containers.contains(&c)
    {
        issues.push(error(IssueKind::ContainerNotSupported {
            family: encoder.family,
            container: c,
        }));
    }
    if let Some(chroma) = chroma {
        let depth = depth(encoder, chroma, wants_hdr, ctx.probe);
        if !encoder.supports_format(depth, chroma) {
            issues.push(error(IssueKind::FormatNotSupported { depth, chroma }));
        } else if let Some(probe) = ctx.probe
            && !probe.supports(&encoder.id, depth, chroma, wants_hdr)
        {
            issues.push(error(IssueKind::NotAvailable));
        }
    }
    if wants_hdr {
        if !matches!(encoder.family, Family::Hevc | Family::Av1) {
            issues.push(error(IssueKind::HdrNeedsHevcOrAv1));
        } else if !encoder.hdr {
            issues.push(error(IssueKind::HdrNotSupported));
        }
    }
    if let Some((w, h)) = size
        && profile.fps > 0
    {
        let rate = u64::from(w) * u64::from(h) * u64::from(profile.fps);
        let max = max_sample_rate(encoder.family);
        if rate > max {
            let max_fps = (max / (u64::from(w) * u64::from(h)).max(1)) as u32;
            issues.push(warning(IssueKind::FramerateTooHigh { max_fps }));
        }
    }
    issues
}

/// The encoder `auto` stands for: the best hardware encoder the probe validated for 8-bit 4:2:0,
/// else libx264. Order: AV1 first (the smallest files for the quality), then HEVC, then H.264;
/// NVIDIA, AMD, Intel…
pub fn pick_auto<'a>(registry: &'a Registry, probe: Option<&ProbeResult>) -> Option<&'a Encoder> {
    let vendor_rank = |v: Vendor| match v {
        Vendor::Nvidia => 0,
        Vendor::Amd => 1,
        Vendor::Intel => 2,
        Vendor::None => 3,
    };
    let family_rank = |f: Family| match f {
        Family::Av1 => 0,
        Family::Hevc => 1,
        Family::H264 => 2,
        Family::Vp9 => 3,
    };
    let best = probe.and_then(|probe| {
        registry
            .encoders()
            .filter(|e| e.kind == Kind::Hardware && probe.supports(&e.id, 8, Chroma::C420, false))
            .min_by_key(|e| (family_rank(e.family), vendor_rank(e.vendor)))
    });
    best.or_else(|| registry.get("libx264"))
}
