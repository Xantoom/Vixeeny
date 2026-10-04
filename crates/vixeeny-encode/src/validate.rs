// SPDX-License-Identifier: GPL-3.0-or-later
//! Validation of a recording profile against the registry (plan 6.4) and the choice of the
//! `auto` encoder. Pure functions: no FFmpeg, no OS.

use vixeeny_common::config::Profile;

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
    BadDepth(u8),
    FormatNotSupported {
        depth: u8,
        chroma: Chroma,
    },
    UnknownHdrSetting(String),
    HdrNeedsHevcOrAv1,
    HdrNeeds10Bit,
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

/// Output size of a `resolution` setting: `source`, `2160p`…`720p` (the ratio is kept), or
/// `WxH`.
pub fn output_size(setting: &str, source: (u32, u32)) -> Option<(u32, u32)> {
    if setting == "source" {
        return Some(source);
    }
    if let Some(lines) = setting.strip_suffix('p') {
        let h: u32 = lines.parse().ok().filter(|h| *h > 0)?;
        let w = (u64::from(source.0) * u64::from(h) / u64::from(source.1.max(1))) as u32;
        return Some((w.max(2) & !1, h & !1));
    }
    let (w, h) = setting.split_once('x')?;
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Whether an audio codec fits a container (annex 13.2); `None` for an unknown codec.
fn audio_ok(codec: &str, container: Container) -> Option<bool> {
    Some(match codec {
        "aac" | "flac" => !matches!(container, Container::Webm),
        "opus" => true,
        "pcm" => matches!(container, Container::Mkv),
        _ => return None,
    })
}

pub fn validate(profile: &Profile, ctx: &Context<'_>) -> Vec<Issue> {
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
    if !matches!(profile.depth, 8 | 10) {
        issues.push(error(IssueKind::BadDepth(profile.depth)));
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
    if wants_hdr {
        if let Some(c) = container
            && c == Container::Webm
        {
            issues.push(error(IssueKind::HdrContainer(c)));
        }
        if profile.depth < 10 {
            issues.push(error(IssueKind::HdrNeeds10Bit));
        }
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
    if let Some(chroma) = chroma
        && matches!(profile.depth, 8 | 10)
    {
        if !encoder.supports_format(profile.depth, chroma) {
            issues.push(error(IssueKind::FormatNotSupported {
                depth: profile.depth,
                chroma,
            }));
        } else if let Some(probe) = ctx.probe
            && !probe.supports(&encoder.id, profile.depth, chroma, wants_hdr)
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

/// Whether the profile has no blocking issue.
pub fn is_valid(issues: &[Issue]) -> bool {
    issues.iter().all(|i| i.severity != Severity::Error)
}

/// The encoder `auto` stands for: the best hardware encoder the probe validated for 8-bit 4:2:0,
/// else libx264. Order: H.264 first (plays everywhere), then HEVC, then AV1; NVIDIA, AMD, Intel…
pub fn pick_auto<'a>(registry: &'a Registry, probe: Option<&ProbeResult>) -> Option<&'a Encoder> {
    let vendor_rank = |v: Vendor| match v {
        Vendor::Nvidia => 0,
        Vendor::Amd => 1,
        Vendor::Intel => 2,
        Vendor::None => 3,
    };
    let family_rank = |f: Family| match f {
        Family::H264 => 0,
        Family::Hevc => 1,
        Family::Av1 => 2,
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
