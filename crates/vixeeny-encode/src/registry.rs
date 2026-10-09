// SPDX-License-Identifier: GPL-3.0-or-later
//! The codec registry (plan 6.1): everything the application knows about encoders is data in
//! `codecs/registry.toml`, embedded in the binary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const REGISTRY: &str = include_str!("../codecs/registry.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    H264,
    Hevc,
    Av1,
    Vp9,
}

impl Family {
    pub const fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Vp9 => "vp9",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Software,
    Hardware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    None,
    Nvidia,
    Amd,
    Intel,
}

/// Registry container names: `mp4` covers the hybrid and the classic MP4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Mkv,
    Mp4,
    Fmp4,
    Webm,
}

impl Container {
    /// Parses the `container` setting of a profile.
    pub fn from_setting(name: &str) -> Option<Self> {
        match name {
            "mkv" => Some(Self::Mkv),
            "mp4" | "mp4_hybrid" => Some(Self::Mp4),
            "fmp4" | "mp4_fragmented" => Some(Self::Fmp4),
            "webm" => Some(Self::Webm),
            _ => None,
        }
    }
}

/// How the custom preset spends bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateMode {
    /// A constant quality; the size follows the picture.
    Quality,
    /// The same quantizer for every frame: simple, and larger than constant quality.
    Cqp,
    /// A target bitrate, allowed to vary.
    Vbr,
    /// The same bitrate all along.
    Cbr,
}

impl RateMode {
    pub const ALL: [Self; 4] = [Self::Quality, Self::Cqp, Self::Vbr, Self::Cbr];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Quality => "quality",
            Self::Cqp => "cqp",
            Self::Vbr => "vbr",
            Self::Cbr => "cbr",
        }
    }

    pub fn from_setting(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// Whether the mode aims at a bitrate (rather than a quality).
    pub const fn has_bitrate(self) -> bool {
        matches!(self, Self::Vbr | Self::Cbr)
    }
}

/// The scale of the constant-quality mode (lower is better).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityScale {
    /// What the encoder calls it: CRF, CQ, QP, ICQ.
    pub name: String,
    pub min: i64,
    pub max: i64,
    pub default: i64,
}

/// The rate control of the custom preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateControl {
    pub quality: QualityScale,
    /// The scale of the constant-quantizer mode, when the encoder has one.
    #[serde(default)]
    pub qp: Option<QualityScale>,
    /// The FFmpeg options of each mode, with the placeholders of [`PLACEHOLDERS`].
    pub modes: BTreeMap<RateMode, BTreeMap<String, OptionValue>>,
}

impl RateControl {
    /// Whether `mode` uses the user's value of `placeholder` (`{maxrate}`, ...).
    pub fn uses(&self, mode: RateMode, placeholder: &str) -> bool {
        self.modes.get(&mode).is_some_and(|options| {
            options
                .values()
                .any(|v| matches!(v, OptionValue::Text(s) if s.contains(placeholder)))
        })
    }
}

/// What the rate-control options of the registry may contain.
pub const PLACEHOLDERS: [&str; 5] = ["{quality}", "{qp}", "{bitrate}", "{maxrate}", "{bufsize}"];

/// The keys of the rate control in the custom options of a profile (`Video::params`); the
/// rates are in kbit/s.
pub mod rate_keys {
    pub const MODE: &str = "rc.mode";
    pub const QUALITY: &str = "rc.quality";
    pub const QP: &str = "rc.qp";
    pub const BITRATE: &str = "rc.bitrate";
    pub const MAXRATE: &str = "rc.maxrate";
}

/// The rates (kbit/s) until the user sets them: fine for 1080p60.
pub const DEFAULT_BITRATE: i64 = 10_000;
pub const DEFAULT_MAXRATE: i64 = 15_000;
/// The range of the bitrate fields (kbit/s).
pub const BITRATE_RANGE: (i64, i64) = (500, 500_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Chroma {
    #[serde(rename = "420")]
    C420,
    #[serde(rename = "422")]
    C422,
    #[serde(rename = "444")]
    C444,
}

impl Chroma {
    pub fn from_setting(name: &str) -> Option<Self> {
        match name {
            "420" => Some(Self::C420),
            "422" => Some(Self::C422),
            "444" => Some(Self::C444),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::C420 => "420",
            Self::C422 => "422",
            Self::C444 => "444",
        }
    }
}

/// A pixel format the encoder accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelFormatSpec {
    pub depth: u8,
    pub chroma: Chroma,
    pub ffmpeg: String,
}

/// An FFmpeg option value written as a TOML scalar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl OptionValue {
    /// The string FFmpeg expects (`1`/`0` for booleans).
    pub fn to_ffmpeg(&self) -> String {
        match self {
            Self::Bool(b) => u8::from(*b).to_string(),
            Self::Int(i) => i.to_string(),
            Self::Float(f) => f.to_string(),
            Self::Text(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetName {
    Quality,
    Balanced,
    Performance,
    Small,
}

impl PresetName {
    pub fn from_setting(name: &str) -> Option<Self> {
        match name {
            "quality" => Some(Self::Quality),
            "balanced" => Some(Self::Balanced),
            "performance" => Some(Self::Performance),
            "small" => Some(Self::Small),
            // The custom preset starts from the balanced one and overrides its options.
            "custom" => Some(Self::Balanced),
            _ => None,
        }
    }
}

/// FFmpeg options of the four simple-mode presets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presets {
    pub quality: BTreeMap<String, OptionValue>,
    pub balanced: BTreeMap<String, OptionValue>,
    pub performance: BTreeMap<String, OptionValue>,
    pub small: BTreeMap<String, OptionValue>,
}

impl Presets {
    pub fn get(&self, name: PresetName) -> &BTreeMap<String, OptionValue> {
        match name {
            PresetName::Quality => &self.quality,
            PresetName::Balanced => &self.balanced,
            PresetName::Performance => &self.performance,
            PresetName::Small => &self.small,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    Enum,
    Int,
    Float,
    Bool,
    String,
}

/// One advanced-mode setting; the UI is generated from these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub key: String,
    pub ffmpeg_option: String,
    #[serde(rename = "type")]
    pub kind: ParamType,
    #[serde(default)]
    pub values: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub default: OptionValue,
    /// The value that leaves the option to FFmpeg (`auto`, or 0 for the key-frame interval).
    pub auto: Option<OptionValue>,
}

impl Param {
    /// The FFmpeg value of `raw` (a stored setting), or `None` when it does not parse.
    fn parse(&self, raw: &str) -> Option<String> {
        match self.kind {
            ParamType::Enum => self.values.iter().any(|v| v == raw).then(|| raw.to_owned()),
            ParamType::Int => raw.parse::<f64>().ok().map(|v| {
                let v = v.round() as i64;
                let lo = self.min.map_or(i64::MIN, |m| m as i64);
                let hi = self.max.map_or(i64::MAX, |m| m as i64);
                v.clamp(lo, hi).to_string()
            }),
            ParamType::Float => raw.parse::<f64>().ok().map(|v| {
                v.clamp(self.min.unwrap_or(f64::MIN), self.max.unwrap_or(f64::MAX))
                    .to_string()
            }),
            ParamType::Bool => match raw {
                "1" | "true" | "on" => Some("1".into()),
                "0" | "false" | "off" => Some("0".into()),
                _ => None,
            },
            ParamType::String => Some(raw.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Encoder {
    pub id: String,
    pub display_name: String,
    pub family: Family,
    pub kind: Kind,
    pub vendor: Vendor,
    pub ffmpeg_encoder: String,
    /// Zero-copy surface types the encoder accepts.
    #[serde(default)]
    pub hw_frames: Vec<String>,
    pub pixel_formats: Vec<PixelFormatSpec>,
    pub hdr: bool,
    pub containers: Vec<Container>,
    pub presets: Presets,
    pub rate_control: RateControl,
    #[serde(default)]
    pub params: Vec<Param>,
}

impl Encoder {
    /// The rate mode of the custom options: the user's when the encoder has it (a constant
    /// bitrate falls back to the variable one), else constant quality.
    pub fn rate_mode(&self, params: &BTreeMap<String, String>) -> RateMode {
        let has = |m: &RateMode| self.rate_control.modes.contains_key(m);
        let wanted = params
            .get(rate_keys::MODE)
            .and_then(|m| RateMode::from_setting(m));
        match wanted {
            Some(m) if has(&m) => m,
            Some(RateMode::Cbr) if has(&RateMode::Vbr) => RateMode::Vbr,
            _ => RateMode::Quality,
        }
    }

    /// The modes the encoder offers, in the order of [`RateMode::ALL`].
    pub fn rate_modes(&self) -> impl Iterator<Item = RateMode> + '_ {
        RateMode::ALL
            .into_iter()
            .filter(|m| self.rate_control.modes.contains_key(m))
    }

    /// The FFmpeg options of the `custom` preset: the options of the chosen rate mode, then
    /// every parameter with the user's value (by key) or its default. Values that do not parse
    /// fall back to the default; numbers are clamped to their range.
    pub fn custom_options(&self, params: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let number = |key: &str, default: i64, (lo, hi): (i64, i64)| {
            params
                .get(key)
                .and_then(|v| v.parse::<f64>().ok())
                .map_or(default, |v| v.round() as i64)
                .clamp(lo, hi)
        };
        let scale = &self.rate_control.quality;
        let mode = self.rate_mode(params);
        let quality = number(rate_keys::QUALITY, scale.default, (scale.min, scale.max));
        let qp = self
            .rate_control
            .qp
            .as_ref()
            .map_or(0, |s| number(rate_keys::QP, s.default, (s.min, s.max)));
        let bitrate = number(rate_keys::BITRATE, DEFAULT_BITRATE, BITRATE_RANGE);
        let maxrate = number(rate_keys::MAXRATE, DEFAULT_MAXRATE, BITRATE_RANGE).max(bitrate);
        let peak = if self.rate_control.uses(mode, "{maxrate}") {
            maxrate
        } else {
            bitrate
        };
        let values = [
            ("{quality}", quality),
            ("{qp}", qp),
            ("{bitrate}", bitrate * 1000),
            ("{maxrate}", maxrate * 1000),
            ("{bufsize}", peak * 2000),
        ];
        let mut options = BTreeMap::new();
        for (key, value) in self.rate_control.modes.get(&mode).into_iter().flatten() {
            let mut value = value.to_ffmpeg();
            for (placeholder, n) in values {
                value = value.replace(placeholder, &n.to_string());
            }
            options.insert(key.clone(), value);
        }
        for param in &self.params {
            let default = param.default.to_ffmpeg();
            let value = params
                .get(&param.key)
                .and_then(|raw| param.parse(raw))
                .or_else(|| param.parse(&default))
                .unwrap_or(default);
            if param.auto.as_ref().is_some_and(|a| a.to_ffmpeg() == value) {
                continue;
            }
            options.insert(param.ffmpeg_option.clone(), value);
        }
        options
    }

    pub fn supports_format(&self, depth: u8, chroma: Chroma) -> bool {
        self.pixel_formats
            .iter()
            .any(|p| p.depth == depth && p.chroma == chroma)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("registry.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("registry.toml: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Registry {
    #[serde(rename = "encoder")]
    pub encoders: Vec<Encoder>,
}

impl Registry {
    /// The registry embedded in the binary.
    pub fn builtin() -> Result<Self, RegistryError> {
        Self::parse(REGISTRY)
    }

    pub fn parse(text: &str) -> Result<Self, RegistryError> {
        let registry: Self = toml::from_str(text)?;
        registry.check()?;
        Ok(registry)
    }

    /// Structural rules the type system cannot express.
    fn check(&self) -> Result<(), RegistryError> {
        let bad = |m: String| Err(RegistryError::Invalid(m));
        let mut seen = std::collections::HashSet::new();
        for e in &self.encoders {
            if !seen.insert(e.id.as_str()) {
                return bad(format!("duplicate id `{}`", e.id));
            }
            if e.pixel_formats.is_empty() || e.containers.is_empty() {
                return bad(format!(
                    "`{}`: pixel_formats and containers must not be empty",
                    e.id
                ));
            }
            if e.kind == Kind::Software && e.vendor != Vendor::None {
                return bad(format!("`{}`: a software encoder has no vendor", e.id));
            }
            if e.kind == Kind::Hardware && e.vendor == Vendor::None {
                return bad(format!("`{}`: a hardware encoder needs a vendor", e.id));
            }
            if e.hdr && !e.pixel_formats.iter().any(|p| p.depth >= 10) {
                return bad(format!("`{}`: hdr needs a 10-bit format", e.id));
            }
            let rc = &e.rate_control;
            let q = &rc.quality;
            if !(q.min <= q.default && q.default <= q.max) {
                return bad(format!("`{}`: the quality default is out of range", e.id));
            }
            if rc.modes.contains_key(&RateMode::Cqp) != rc.qp.is_some() {
                return bad(format!(
                    "`{}`: the cqp mode and the qp scale go together",
                    e.id
                ));
            }
            if !rc.modes.contains_key(&RateMode::Quality) {
                return bad(format!("`{}`: no constant-quality mode", e.id));
            }
            for value in rc.modes.values().flat_map(|o| o.values()) {
                if let OptionValue::Text(s) = value {
                    let rest = PLACEHOLDERS.iter().fold(s.clone(), |s, p| s.replace(p, ""));
                    if rest.contains('{') {
                        return bad(format!("`{}`: unknown placeholder in `{s}`", e.id));
                    }
                }
            }
            for p in &e.params {
                if p.kind == ParamType::Enum && p.values.is_empty() {
                    return bad(format!("`{}`.{}: enum without values", e.id, p.key));
                }
                if p.key.starts_with("rc.") {
                    return bad(format!(
                        "`{}`.{}: `rc.` keys are the rate control's",
                        e.id, p.key
                    ));
                }
                if p.parse(&p.default.to_ffmpeg()).as_deref() != Some(&p.default.to_ffmpeg()) {
                    return bad(format!("`{}`.{}: bad default", e.id, p.key));
                }
                if let (Some(lo), Some(hi)) = (p.min, p.max)
                    && lo > hi
                {
                    return bad(format!("`{}`.{}: min > max", e.id, p.key));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Encoder> {
        self.encoders.iter().find(|e| e.id == id)
    }

    pub fn encoders(&self) -> impl Iterator<Item = &Encoder> {
        self.encoders.iter()
    }
}
