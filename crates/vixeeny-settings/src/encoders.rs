// SPDX-License-Identifier: GPL-3.0-or-later
//! Which encoders the video page offers: the ones of the codec registry that exist on this
//! platform and, for the hardware ones, that the probe opened a session with.

use std::sync::LazyLock;

use vixeeny_common::config::Video;
use vixeeny_common::i18n::Lang;
use vixeeny_encode::registry::{Encoder, Family, Kind, RateMode, Registry, Vendor};

use crate::Env;

static REGISTRY: LazyLock<Option<Registry>> = LazyLock::new(|| Registry::builtin().ok());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    pub id: String,
    pub name: String,
    pub hardware: bool,
    /// Lower is better: for the hardware ones the codec (AV1, HEVC, H.264), then the vendor
    /// (the order `pick_auto` uses); for the software ones the registry's order (x264 first:
    /// an AV1 encoder on the processor cannot keep up with a game in real time).
    rank: (u8, u8),
}

pub fn registry() -> Option<&'static Registry> {
    REGISTRY.as_ref()
}

/// The encoders, hardware before software, each kind in the order of the codecs (H.264, HEVC,
/// VP9, AV1), then of the vendors.
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
    let shown = |f: Family| match f {
        Family::H264 => 0,
        Family::Hevc => 1,
        Family::Vp9 => 2,
        Family::Av1 => 3,
    };
    let mut list: Vec<(EncoderInfo, (u8, u8))> = registry
        .encoders()
        .enumerate()
        .map(|(i, e)| {
            let hardware = e.kind == Kind::Hardware;
            let rank = if hardware {
                (family(e.family), vendor(e.vendor))
            } else {
                (0, i as u8)
            };
            let info = EncoderInfo {
                id: e.id.clone(),
                name: e.display_name.clone(),
                hardware,
                rank,
            };
            (info, (shown(e.family), vendor(e.vendor)))
        })
        .collect();
    list.sort_by_key(|(e, order)| (!e.hardware, *order));
    list.into_iter().map(|(e, _)| e).collect()
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

/// The best usable encoder of one kind (the one recommended).
pub fn best(env: &Env, hardware: bool) -> Option<&EncoderInfo> {
    choices(env, hardware).into_iter().min_by_key(|e| e.rank)
}

/// The encoder a profile uses: its own when it is listed for its kind, else the best one.
pub fn resolved<'a>(env: &'a Env, profile: &Video) -> Option<&'a EncoderInfo> {
    let hardware = profile.encoder_kind != "software";
    choices(env, hardware)
        .into_iter()
        .find(|e| e.id == profile.encoder)
        .or_else(|| best(env, hardware))
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

/// The hint of the quality and quantizer sliders.
pub fn lower_is_better(lang: Lang) -> &'static str {
    if lang == Lang::Fr {
        "Plus bas = meilleure image, fichiers plus gros."
    } else {
        "Lower = better picture, larger files."
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

/// The label of a rate mode, with what the encoder calls it (CRF, CQ...).
pub fn rate_mode_label(mode: RateMode, encoder: &Encoder, lang: Lang) -> String {
    let fr = lang == Lang::Fr;
    match mode {
        RateMode::Quality => {
            let scale = &encoder.rate_control.quality.name;
            if fr {
                format!("Qualité constante ({scale})")
            } else {
                format!("Constant quality ({scale})")
            }
        }
        RateMode::Cqp => if fr {
            "QP constant (CQP)"
        } else {
            "Constant QP (CQP)"
        }
        .to_owned(),
        RateMode::Vbr => if fr {
            "Débit variable (VBR)"
        } else {
            "Variable bitrate (VBR)"
        }
        .to_owned(),
        RateMode::Cbr => if fr {
            "Débit constant (CBR)"
        } else {
            "Constant bitrate (CBR)"
        }
        .to_owned(),
    }
}

/// `(min, max, step)` of a numeric registry parameter.
pub fn param_range(min: Option<f64>, max: Option<f64>) -> (i64, i64, i64) {
    let lo = min.unwrap_or(0.0) as i64;
    let hi = max.unwrap_or(100.0) as i64;
    let step = if hi - lo > 1000 { 10 } else { 1 };
    (lo, hi, step)
}
