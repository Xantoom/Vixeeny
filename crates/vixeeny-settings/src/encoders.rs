// SPDX-License-Identifier: GPL-3.0-or-later
//! Which encoders the video page offers: the ones of the codec registry that exist on this
//! platform and, for the hardware ones, that the probe opened a session with.

use std::sync::LazyLock;

use vixeeny_common::config::Video;
use vixeeny_common::i18n::Lang;
use vixeeny_encode::registry::{Chroma, Encoder, Family, Kind, RateMode, Registry, Vendor};

use crate::Env;

static REGISTRY: LazyLock<Option<Registry>> = LazyLock::new(|| Registry::builtin().ok());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    pub id: String,
    pub name: String,
    pub hardware: bool,
    /// Lower is better: the codec (AV1, HEVC, H.264), then the vendor (the order `pick_auto`
    /// uses).
    rank: (u8, u8),
}

pub fn registry() -> Option<&'static Registry> {
    REGISTRY.as_ref()
}

/// The encoders, hardware before software, best first.
pub fn all() -> Vec<EncoderInfo> {
    let Some(registry) = registry() else {
        return Vec::new();
    };
    let family = |f: Family| match f {
        Family::Av1 => 0,
        Family::Hevc => 1,
        Family::H264 => 2,
        Family::Vp9 => 3,
    };
    let vendor = |v: Vendor| match v {
        Vendor::Nvidia => 0,
        Vendor::Amd => 1,
        Vendor::Intel => 2,
        Vendor::None => 3,
    };
    let mut list: Vec<EncoderInfo> = registry
        .encoders()
        .map(|e| EncoderInfo {
            id: e.id.clone(),
            name: e.display_name.clone(),
            hardware: e.kind == Kind::Hardware,
            rank: (family(e.family), vendor(e.vendor)),
        })
        .collect();
    // Stable: software keeps the registry's order (x264, x265, VP9, SVT-AV1): an AV1 encoder
    // on the processor cannot keep up with a game in real time.
    list.sort_by_key(|e| (!e.hardware, if e.hardware { e.rank } else { (0, 0) }));
    list
}

/// Whether `id` can be used now: software always, hardware once the probe opened it.
fn usable(env: &Env, info: &EncoderInfo) -> bool {
    !info.hardware || env.probe.as_ref().is_some_and(|p| p.is_available(&info.id))
}

/// The encoders of one kind that are usable on this computer, best first.
pub fn choices(env: &Env, hardware: bool) -> Vec<&EncoderInfo> {
    env.encoders
        .iter()
        .filter(|e| e.hardware == hardware && usable(env, e))
        .collect()
}

/// The encoder a profile uses: its own when it is listed for its kind, else the best one.
pub fn resolved<'a>(env: &'a Env, profile: &Video) -> Option<&'a EncoderInfo> {
    let list = choices(env, profile.encoder_kind != "software");
    list.iter()
        .find(|e| e.id == profile.encoder)
        .copied()
        .or_else(|| list.first().copied())
}

/// Whether the encoder can do 10-bit 4:2:0 (what the 10-bit switch turns on).
pub fn ten_bit(env: &Env, encoder: &str) -> bool {
    let Some(spec) = spec(encoder) else {
        return false;
    };
    if !spec.supports_format(10, Chroma::C420) {
        return false;
    }
    match (&env.probe, spec.kind) {
        (Some(probe), Kind::Hardware) => probe.supports(encoder, 10, Chroma::C420, false),
        _ => true,
    }
}

pub fn spec(id: &str) -> Option<&'static Encoder> {
    registry().and_then(|r| r.get(id))
}

/// The label of an encoder option, by its registry key.
pub fn param_label(key: &str, lang: Lang) -> String {
    let (en, fr) = match key {
        "rc.mode" => ("Rate control", "Contrôle du débit"),
        "rc.bitrate" => ("Bitrate (kbit/s)", "Débit (kbit/s)"),
        "rc.maxrate" => ("Maximum bitrate (kbit/s)", "Débit maximal (kbit/s)"),
        "preset" => ("Speed / quality preset", "Préréglage vitesse / qualité"),
        "tune" => ("Tuning", "Optimisation"),
        "quality" => ("Quality preset", "Préréglage de qualité"),
        "multipass" => ("Two-pass encoding", "Encodage en deux passes"),
        "keyint" => (
            "Key-frame interval (frames, 0 = auto)",
            "Intervalle d'images clés (images, 0 = auto)",
        ),
        "bframes" => ("B-frames", "Images B"),
        "lookahead" => ("Look-ahead (frames)", "Anticipation (images)"),
        "spatial-aq" => (
            "Spatial adaptive quantization",
            "Quantification adaptative spatiale",
        ),
        "temporal-aq" => (
            "Temporal adaptive quantization",
            "Quantification adaptative temporelle",
        ),
        "vbaq" => (
            "Variance-based adaptive quantization",
            "Quantification adaptative (VBAQ)",
        ),
        "preanalysis" => ("Pre-analysis", "Pré-analyse"),
        "row-mt" => ("Multi-threaded rows", "Lignes multi-thread"),
        "deadline" => ("Quality / speed", "Qualité / vitesse"),
        "cpu-used" => (
            "Speed (-8 = slowest, 8 = fastest)",
            "Vitesse (-8 = la plus lente, 8 = la plus rapide)",
        ),
        other => return other.to_owned(),
    };
    if lang == Lang::Fr { fr } else { en }.to_owned()
}

/// The label of the constant-quality slider: `scale` is what the encoder calls it (CRF, CQ...).
pub fn quality_label(scale: &str, lang: Lang) -> String {
    if lang == Lang::Fr {
        format!("Qualité ({scale}, plus bas = meilleur)")
    } else {
        format!("Quality ({scale}, lower = better)")
    }
}

/// The label of a numbered preset (SVT-AV1).
pub fn numbered_preset_label(min: i64, max: i64, lang: Lang) -> String {
    if lang == Lang::Fr {
        format!("Préréglage ({min} = le plus lent, {max} = le plus rapide)")
    } else {
        format!("Preset ({min} = slowest, {max} = fastest)")
    }
}

/// The label of one value of an encoder option.
pub fn value_label(key: &str, value: &str, lang: Lang) -> String {
    let pair = match (key, value) {
        (_, "auto") => ("Auto", "Auto"),
        // NVENC: P1 to P7. Above P5 the encoder struggles with 4K at 60 fps.
        ("preset", "p5") => ("P5 (recommended)", "P5 (recommandé)"),
        ("preset", p) if p.len() == 2 && p.starts_with('p') => return p.to_uppercase(),
        // x264, x265, Quick Sync.
        ("preset", "ultrafast") => ("Ultra fast", "Ultra rapide"),
        ("preset", "superfast") => ("Super fast", "Super rapide"),
        ("preset", "veryfast") => ("Very fast", "Très rapide"),
        ("preset", "faster") => ("Faster", "Plus rapide"),
        ("preset", "fast") => ("Fast", "Rapide"),
        ("preset", "medium") => ("Medium", "Moyen"),
        ("preset", "slow") => ("Slow", "Lent"),
        ("preset", "slower") => ("Slower", "Plus lent"),
        ("preset", "veryslow") => ("Very slow", "Très lent"),
        ("preset", "placebo") => ("Placebo", "Placebo"),
        ("tune", "film") => ("Film", "Film"),
        ("tune", "animation") => ("Animation", "Animation"),
        ("tune", "grain") => ("Grain", "Grain"),
        ("tune", "stillimage") => ("Still image", "Image fixe"),
        ("tune", "fastdecode") => ("Fast decoding", "Décodage rapide"),
        ("tune", "psnr") => ("PSNR", "PSNR"),
        ("tune", "ssim") => ("SSIM", "SSIM"),
        ("multipass", "disabled") => ("Off", "Désactivé"),
        ("multipass", "qres") => (
            "Quarter resolution (faster)",
            "Quart de résolution (plus rapide)",
        ),
        ("multipass", "fullres") => (
            "Full resolution (more precise)",
            "Pleine résolution (plus précis)",
        ),
        ("quality", "speed") => ("Speed", "Vitesse"),
        ("quality", "balanced") => ("Balanced", "Équilibré"),
        ("quality", "quality") => ("Quality", "Qualité"),
        ("deadline", "best") => ("Best (very slow)", "Meilleure (très lent)"),
        ("deadline", "good") => ("Good", "Bonne"),
        ("deadline", "realtime") => ("Real time", "Temps réel"),
        _ => return value.to_owned(),
    };
    if lang == Lang::Fr { pair.1 } else { pair.0 }.to_owned()
}

/// The label of a rate mode.
pub fn rate_mode_label(mode: RateMode, lang: Lang) -> &'static str {
    let (en, fr) = match mode {
        RateMode::Quality => ("Constant quality", "Qualité constante"),
        RateMode::Vbr => ("Variable bitrate (VBR)", "Débit variable (VBR)"),
    };
    if lang == Lang::Fr { fr } else { en }
}

/// `(min, max, step)` of a numeric registry parameter.
pub fn param_range(min: Option<f64>, max: Option<f64>) -> (i64, i64, i64) {
    let lo = min.unwrap_or(0.0) as i64;
    let hi = max.unwrap_or(100.0) as i64;
    let step = if hi - lo > 1000 { 10 } else { 1 };
    (lo, hi, step)
}
