// SPDX-License-Identifier: GPL-3.0-or-later
//! Which encoders the video page offers: the ones of the codec registry that exist on this
//! platform and, for the hardware ones, that the probe opened a session with.

use std::sync::LazyLock;

use vixeeny_common::config::Profile;
use vixeeny_common::i18n::Lang;
use vixeeny_encode::registry::{Chroma, Encoder, Family, Kind, ParamType, Registry, Vendor};

use crate::Env;

static REGISTRY: LazyLock<Option<Registry>> = LazyLock::new(|| Registry::builtin().ok());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    pub id: String,
    pub name: String,
    pub hardware: bool,
    /// Lower is better: the codec first, then the vendor (the order `pick_auto` uses).
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
        Family::H264 => 0,
        Family::Hevc => 1,
        Family::Av1 => 2,
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
    // Stable: software keeps the registry's order (x264, x265, AV1, VP9).
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
pub fn resolved<'a>(env: &'a Env, profile: &Profile) -> Option<&'a EncoderInfo> {
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
        "preset" => (
            "Speed / efficiency preset",
            "Préréglage vitesse / efficacité",
        ),
        "tune" => ("Tuning", "Optimisation"),
        "profile" => ("Profile", "Profil"),
        "crf" => (
            "Quality (CRF, lower = better)",
            "Qualité (CRF, plus bas = meilleur)",
        ),
        "cq" => (
            "Quality (CQ, lower = better)",
            "Qualité (CQ, plus bas = meilleur)",
        ),
        "qp" => (
            "Quantizer (QP, lower = better)",
            "Quantificateur (QP, plus bas = meilleur)",
        ),
        "qp_i" => ("Quantizer of key frames", "Quantificateur des images clés"),
        "qp_p" => (
            "Quantizer of other frames",
            "Quantificateur des autres images",
        ),
        "global_quality" => ("Quality (lower = better)", "Qualité (plus bas = meilleur)"),
        "quality" => ("Quality level", "Niveau de qualité"),
        "rc" | "rc_mode" => ("Rate control", "Contrôle du débit"),
        "usage" => ("Usage", "Usage"),
        "multipass" => ("Multi-pass encoding", "Encodage multi-passes"),
        "keyint" => (
            "Key frame interval (frames)",
            "Intervalle d'images clés (images)",
        ),
        "bframes" => ("B-frames", "Images B"),
        "lookahead" => ("Look-ahead (frames)", "Anticipation (images)"),
        "low_power" => ("Low-power mode", "Mode basse consommation"),
        "spatial-aq" => ("Adaptive quantization", "Quantification adaptative"),
        "realtime" => ("Real-time mode", "Mode temps réel"),
        "row-mt" => ("Multi-threaded rows", "Lignes multi-thread"),
        "deadline" => ("Deadline", "Délai d'encodage"),
        "cpu-used" => (
            "CPU effort (higher = faster)",
            "Effort CPU (plus haut = plus rapide)",
        ),
        other => return other.to_owned(),
    };
    if lang == Lang::Fr { fr } else { en }.to_owned()
}

/// `(min, max, step)` of a numeric registry parameter.
pub fn param_range(kind: ParamType, min: Option<f64>, max: Option<f64>) -> (i64, i64, i64) {
    let lo = min.unwrap_or(0.0) as i64;
    let hi = max.unwrap_or(100.0) as i64;
    let step = if hi - lo >= 1000 { 10 } else { 1 };
    let _ = kind;
    (lo, hi, step)
}
