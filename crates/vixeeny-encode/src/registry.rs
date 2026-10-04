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
    Apple,
    Vaapi,
    Vulkan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    Macos,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateControl {
    Crf,
    Cqp,
    Vbr,
    Cbr,
}

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
    /// Translation key.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Encoder {
    pub id: String,
    pub display_name: String,
    pub family: Family,
    pub kind: Kind,
    pub vendor: Vendor,
    pub ffmpeg_encoder: String,
    pub platforms: Vec<Platform>,
    /// Zero-copy surface types the encoder accepts.
    #[serde(default)]
    pub hw_frames: Vec<String>,
    pub pixel_formats: Vec<PixelFormatSpec>,
    pub hdr: bool,
    pub containers: Vec<Container>,
    pub rate_control: Vec<RateControl>,
    pub presets: Presets,
    #[serde(default)]
    pub params: Vec<Param>,
}

impl Encoder {
    /// The FFmpeg options of the `custom` preset: the balanced preset with the user's values
    /// (by parameter key) on top. A value that does not parse for its parameter is ignored.
    pub fn custom_options(
        &self,
        params: &std::collections::BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let mut options: BTreeMap<String, String> = self
            .presets
            .balanced
            .iter()
            .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
            .collect();
        for param in &self.params {
            let Some(raw) = params.get(&param.key) else {
                continue;
            };
            let value = match param.kind {
                ParamType::Enum => param.values.iter().any(|v| v == raw).then(|| raw.clone()),
                ParamType::Int => raw.parse::<i64>().ok().map(|v| v.to_string()),
                ParamType::Float => raw.parse::<f64>().ok().map(|v| v.to_string()),
                ParamType::Bool => match raw.as_str() {
                    "1" | "true" | "on" => Some("1".into()),
                    "0" | "false" | "off" => Some("0".into()),
                    _ => None,
                },
                ParamType::String => Some(raw.clone()),
            };
            if let Some(value) = value {
                options.insert(param.ffmpeg_option.clone(), value);
            }
        }
        options
    }

    pub fn supports_format(&self, depth: u8, chroma: Chroma) -> bool {
        self.pixel_formats
            .iter()
            .any(|p| p.depth == depth && p.chroma == chroma)
    }

    pub fn on(&self, platform: Platform) -> bool {
        self.platforms.contains(&platform)
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
            if e.pixel_formats.is_empty() || e.containers.is_empty() || e.platforms.is_empty() {
                return bad(format!(
                    "`{}`: pixel_formats, containers and platforms must not be empty",
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
            for p in &e.params {
                if p.kind == ParamType::Enum && p.values.is_empty() {
                    return bad(format!("`{}`.{}: enum without values", e.id, p.key));
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

    /// Encoders usable on `platform`.
    pub fn for_platform(&self, platform: Platform) -> impl Iterator<Item = &Encoder> {
        self.encoders.iter().filter(move |e| e.on(platform))
    }
}
