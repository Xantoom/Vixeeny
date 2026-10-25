// SPDX-License-Identifier: GPL-3.0-or-later
//! The codec registry: everything the application knows about encoders is data in `codecs/registry.toml`, embedded in
//! the binary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::probe::NvencGeneration;

pub(crate) const REGISTRY: &str = include_str!("../codecs/registry.toml");

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

    /// The family of a setting's name (`h264`, `hevc`, `av1`, `vp9`).
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::H264, Self::Hevc, Self::Av1, Self::Vp9].into_iter().find(|f| f.name() == name)
    }

    /// Hardware encoders are preferred in this order: AV1 (the smallest files for the quality), HEVC, H.264, VP9.
    pub const fn preference(self) -> u8 {
        match self {
            Self::Av1 => 0,
            Self::Hevc => 1,
            Self::H264 => 2,
            Self::Vp9 => 3,
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

impl Vendor {
    /// NVIDIA, AMD, Intel, then none.
    pub const fn preference(self) -> u8 {
        match self {
            Self::Nvidia => 0,
            Self::Amd => 1,
            Self::Intel => 2,
            Self::None => 3,
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

/// How the custom preset spends bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateMode {
    /// A constant quality; the size follows the picture.
    Quality,
    /// A target bitrate, allowed to vary.
    Vbr,
    /// The same bitrate all along.
    Cbr,
}

impl RateMode {
    pub const ALL: [Self; 3] = [Self::Quality, Self::Vbr, Self::Cbr];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Quality => "quality",
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
    /// What the encoder calls its quality mode: CRF, CQ, ICQ, CQP.
    pub name: String,
    pub min: i64,
    pub max: i64,
    pub default: i64,
    /// The encoder's own scale runs the other way (AMF's QVBR: 51 is the best): the user's value, lower is better as
    /// everywhere else, is mirrored into it.
    #[serde(default)]
    pub inverted: bool,
}

impl QualityScale {
    /// The encoder's value of `quality`, a value of the user's scale.
    pub fn to_encoder(&self, quality: i64) -> i64 {
        if self.inverted { self.min + self.max - quality } else { quality }
    }
}

/// The rate control of the custom preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateControl {
    pub quality: QualityScale,
    /// The FFmpeg options of each mode, with the placeholders of [`PLACEHOLDERS`].
    pub modes: BTreeMap<RateMode, BTreeMap<String, OptionValue>>,
}

impl RateControl {
    /// Whether `mode` uses the user's value of `placeholder` (`{maxrate}`, ...).
    pub fn uses(&self, mode: RateMode, placeholder: &str) -> bool {
        self.modes.get(&mode).is_some_and(|options| {
            options.values().any(|v| matches!(v, OptionValue::Text(s) if s.contains(placeholder)))
        })
    }
}

/// What the rate-control options of the registry may contain.
pub const PLACEHOLDERS: [&str; 4] = ["{quality}", "{bitrate}", "{maxrate}", "{bufsize}"];

/// The keys of the rate control in the custom options of a profile (`Video::params`); the rates are in kbit/s.
pub mod rate_keys {
    pub const MODE: &str = "rc.mode";
    pub const QUALITY: &str = "rc.quality";
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
}

impl PresetName {
    pub fn from_setting(name: &str) -> Option<Self> {
        match name {
            "quality" => Some(Self::Quality),
            "balanced" => Some(Self::Balanced),
            "performance" => Some(Self::Performance),
            // Before 1.0: what balanced is now.
            "small" => Some(Self::Balanced),
            // The custom preset starts from the balanced one and overrides its options.
            "custom" => Some(Self::Balanced),
            _ => None,
        }
    }
}

/// FFmpeg options of the three simple-mode presets: balanced for recordings, quality for editing and keeping,
/// performance (balanced's level without the tools that load the encoder: look-ahead, B-frames, extra pass, adaptive
/// quantisation) for the replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presets {
    pub quality: BTreeMap<String, OptionValue>,
    pub balanced: BTreeMap<String, OptionValue>,
    pub performance: BTreeMap<String, OptionValue>,
}

impl Presets {
    pub fn get(&self, name: PresetName) -> &BTreeMap<String, OptionValue> {
        match name {
            PresetName::Quality => &self.quality,
            PresetName::Balanced => &self.balanced,
            PresetName::Performance => &self.performance,
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
            ParamType::Float => raw
                .parse::<f64>()
                .ok()
                .map(|v| v.clamp(self.min.unwrap_or(f64::MIN), self.max.unwrap_or(f64::MAX)).to_string()),
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
    /// The rate mode of the custom options: the user's when the encoder has it (a constant bitrate falls back to the
    /// variable one), else constant quality.
    pub fn rate_mode(&self, params: &BTreeMap<String, String>) -> RateMode {
        let has = |m: &RateMode| self.rate_control.modes.contains_key(m);
        let wanted = params.get(rate_keys::MODE).and_then(|m| RateMode::from_setting(m));
        match wanted {
            Some(m) if has(&m) => m,
            Some(RateMode::Cbr) if has(&RateMode::Vbr) => RateMode::Vbr,
            _ => RateMode::Quality,
        }
    }

    /// The modes the encoder offers, in the order of [`RateMode::ALL`].
    pub fn rate_modes(&self) -> impl Iterator<Item = RateMode> + '_ {
        RateMode::ALL.into_iter().filter(|m| self.rate_control.modes.contains_key(m))
    }

    /// The FFmpeg options of the `custom` preset: the options of the chosen rate mode, then every parameter with the
    /// user's value (by key) or its default. Values that do not parse fall back to the default; numbers are clamped to
    /// their range.
    pub fn custom_options(&self, params: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let number = |key: &str, default: i64, (lo, hi): (i64, i64)| {
            params.get(key).and_then(|v| v.parse::<f64>().ok()).map_or(default, |v| v.round() as i64).clamp(lo, hi)
        };
        let scale = &self.rate_control.quality;
        let mode = self.rate_mode(params);
        // A value off the scale was set on another one (the scale changed): the default.
        let quality = params
            .get(rate_keys::QUALITY)
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v.round() as i64)
            .filter(|q| (scale.min..=scale.max).contains(q))
            .unwrap_or(scale.default);
        let quality = scale.to_encoder(quality);
        let bitrate = number(rate_keys::BITRATE, DEFAULT_BITRATE, BITRATE_RANGE);
        let maxrate = number(rate_keys::MAXRATE, DEFAULT_MAXRATE, BITRATE_RANGE).max(bitrate);
        let peak = if self.rate_control.uses(mode, "{maxrate}") { maxrate } else { bitrate };
        let values = [
            ("{quality}", quality),
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

    /// The custom settings that reproduce `preset`, where the parameters allow: constant quality at its level, and each
    /// parameter the preset sets (the others keep their default).
    pub fn preset_params(&self, preset: PresetName) -> BTreeMap<String, String> {
        let options = self.presets.get(preset);
        let mut params = BTreeMap::from([(rate_keys::MODE.to_owned(), RateMode::Quality.name().to_owned())]);
        let scale = &self.rate_control.quality;
        let level = self
            .rate_control
            .modes
            .get(&RateMode::Quality)
            .into_iter()
            .flatten()
            .filter(|(_, v)| v.to_ffmpeg().contains("{quality}"))
            .find_map(|(k, _)| options.get(k)?.to_ffmpeg().parse::<i64>().ok());
        if let Some(level) = level {
            // The mirror of a mirrored scale is the user's value.
            params.insert(rate_keys::QUALITY.to_owned(), scale.to_encoder(level).to_string());
        }
        for param in &self.params {
            if let Some(value) = options.get(&param.ffmpeg_option).and_then(|v| param.parse(&v.to_ffmpeg())) {
                params.insert(param.key.clone(), value);
            }
        }
        params
    }

    /// The speed option that keeps up with `pixel_rate` (width × height × frames a second), and its value: the best one
    /// that still keeps up, with a margin, while a game runs.
    ///
    /// NVENC: frames a second at 1080p of one encoder of its generation ([`nvenc_speeds`]). P5 at most: P6 and P7 cost
    /// two to three times more for 1 to 3 % (SSIMULACRA 2, VMAF). H.264 starts at P4, as good as P5 there and half as
    /// fast again. Where none keeps up, the fastest, which [`Encoder::fit_tools`] may split between the encoders.
    ///
    /// AMD: the best preset whose throughput, as the driver gives it for this engine
    /// ([`Tools::amf_throughput`](crate::probe::Tools)), keeps up. Without it, and on Intel, from the speed of the
    /// media engine at its default preset ([`Tools::engine_fps`], measured per generation); AMF's quality preset is
    /// then taken as halving it (rigaya's quality settings, which add the pre-analysis, cost 40 to 75 % on H.264 and
    /// AV1).
    pub fn speed_for(&self, pixel_rate: u64, tools: Option<crate::probe::Tools>) -> Option<(&'static str, String)> {
        let needed = frames_needed(pixel_rate);
        let engine = f64::from(tools.map_or(0, |t| t.engine_fps));
        let amf_throughput = tools.map_or([0; 4], |t| t.amf_throughput);
        let pick = |key: &'static str, steps: &[(&str, f64)]| {
            let value = steps.iter().find(|(_, fps)| *fps >= needed).or(steps.last()).map(|(v, _)| (*v).to_owned())?;
            Some((key, value))
        };
        let nvenc = self.id.starts_with("nvenc");
        let amf = self.id.starts_with("amf");
        let qsv = self.id.starts_with("qsv");
        match (self.family, self.id.as_str()) {
            _ if nvenc => pick("preset", nvenc_speeds(self.family, tools.map(|t| t.generation).unwrap_or_default())),
            _ if amf && amf_throughput[..3].iter().all(|&t| t > 0) => {
                let [_, balanced, quality, _] = amf_throughput.map(frames_of_macroblocks);
                pick("quality", &[("quality", quality), ("balanced", balanced), ("speed", f64::MAX)])
            }
            _ if amf && engine > 0.0 => {
                pick("quality", &[("quality", engine * AMF_QUALITY_SHARE), ("balanced", engine), ("speed", f64::MAX)])
            }
            _ if qsv && engine > 0.0 => {
                pick("preset", &[("medium", engine), ("fast", engine * 1.5), ("veryfast", f64::MAX)])
            }
            // Not measured (no probe): their media engines are rated for 4K at 60 FPS, so the best preset up to that,
            // one step faster above.
            _ if amf => pick("quality", &[("quality", 312.0), ("balanced", 624.0), ("speed", f64::MAX)]),
            _ if qsv => pick("preset", &[("medium", 312.0), ("fast", 624.0), ("veryfast", f64::MAX)]),
            // Software encoders share the processor with the game.
            (_, "libx264") => pick("preset", &[("veryfast", 78.0), ("superfast", 156.0), ("ultrafast", f64::MAX)]),
            (_, "libx265") => pick("preset", &[("superfast", 78.0), ("ultrafast", f64::MAX)]),
            (_, "libsvtav1") => pick("preset", &[("10", 78.0), ("11", 156.0), ("12", 312.0), ("13", f64::MAX)]),
            (_, "libvpx_vp9") => pick("cpu-used", &[("6", 78.0), ("7", 156.0), ("8", f64::MAX)]),
            _ => None,
        }
    }

    /// The FFmpeg options of a simple preset for a recording of `size` at `fps`: the encoder's speed follows the number
    /// of pixels to encode, and from 4K on the quality scale eases a little (the same look, much lighter files).
    pub fn preset_options(
        &self,
        preset: PresetName,
        (width, height): (u32, u32),
        fps: u32,
        tools: Option<crate::probe::Tools>,
    ) -> BTreeMap<String, String> {
        let mut options: BTreeMap<String, String> =
            self.presets.get(preset).iter().map(|(k, v)| (k.clone(), v.to_ffmpeg())).collect();
        let rate = u64::from(width) * u64::from(height) * u64::from(fps.max(1));
        // The performance preset of a software encoder keeps its own speed (there, the speed is the tools).
        if (preset != PresetName::Performance || self.kind == Kind::Hardware)
            && let Some((key, value)) = self.speed_for(rate, tools)
            && let Some(param) = self.params.iter().find(|p| p.key == key)
            && options.contains_key(&param.ffmpeg_option)
        {
            options.insert(param.ffmpeg_option.clone(), value);
        }
        if height >= 2000 {
            let scale = &self.rate_control.quality;
            // Two steps of a 0-51 scale, as much on the others: at the same level a 4K picture measures 2 to 3 steps
            // better per pixel than 1080p (SSIMULACRA 2), for 15 % less.
            let step = 2 * ((scale.max - scale.min) as f64 / 51.0).round().max(1.0) as i64;
            let ease = if scale.inverted { -step } else { step };
            let quality_options: Vec<String> = self
                .rate_control
                .modes
                .get(&RateMode::Quality)
                .into_iter()
                .flatten()
                .filter(|(_, v)| v.to_ffmpeg().contains("{quality}"))
                .map(|(k, _)| k.clone())
                .collect();
            for key in quality_options {
                if let Some(value) = options.get_mut(&key)
                    && let Ok(n) = value.parse::<i64>()
                {
                    *value = (n + ease).clamp(scale.min, scale.max).to_string();
                }
            }
        }
        options
    }

    /// Holds constant-quality `options` to `kbps` at most (kbit/s), over the two seconds of buffer of the custom rates:
    /// the encoder keeps its quality until a scene needs more. The bitrate modes keep their own rates. QSV needs a
    /// target under the ceiling to take it (FFmpeg then picks QVBR); VP9 takes it as its constrained quality; QSV with
    /// look-ahead, its own constant-quality mode, has none.
    pub fn cap_rate(&self, options: &mut BTreeMap<String, String>, kbps: u32) {
        if !["cq", "qvbr_quality_level", "global_quality", "crf"].iter().any(|k| options.contains_key(*k)) {
            return;
        }
        let bits = u64::from(kbps.max(1)) * 1000;
        options.insert("maxrate".into(), bits.to_string());
        options.insert("bufsize".into(), (bits * 2).to_string());
        if options.contains_key("global_quality") {
            options.insert("b".into(), (bits * 3 / 4).to_string());
        } else if self.family == Family::Vp9 {
            options.insert("b".into(), bits.to_string());
        }
    }

    /// Fits FFmpeg `options` to the tools of the GPU's encoder (`tools`: its driver's answer; `None`, not described:
    /// they are left as they are).
    ///
    /// With `choose`, what the options leave open is chosen as NVIDIA advises for recording (Video Codec SDK 13.1
    /// programming guide, recommended settings; NVIDIA's broadcasting guide): the look-ahead on, [`LOOKAHEAD_FRAMES`]
    /// of it, which then picks up to 4 B-frames for each group, else 2 B-frames. Without `choose` (the performance
    /// preset), the encoder's own.
    ///
    /// What the encoder lacks is left out rather than refused when it opens: B-frames beyond its most (none for HEVC on
    /// Pascal), B-frames as references (and always for H.264, which some decoders play wrong with them, as OBS found),
    /// the look-ahead, the temporal AQ.
    ///
    /// AMF: B-frames cost about 40 % of the engine's speed (RX 6900 XT, H.264: 139 to 84 FPS for 1.3 VMAF), the
    /// pre-analysis two thirds (84 to 26 FPS for 0.4 to 0.7 VMAF), so the latter is never chosen, and 2 B-frames (AMD's
    /// advice for recording) only when the engine has that much to spare at `pixel_rate` beside the quality preset.
    pub fn fit_tools(
        &self,
        options: &mut BTreeMap<String, String>,
        tools: Option<crate::probe::Tools>,
        choose: bool,
        pixel_rate: u64,
    ) {
        // Constant quality with no ceiling of its own is held to one all the same: NVENC's, of the smallest level the
        // picture fits (HEVC at 1080p60: about 17 Mbit/s, which turns the busiest scenes of the quality presets to
        // blocks), AMF's 30 Mbit/s. Half a bit per pixel leaves the quality to the encoder; NVENC then signals the
        // level that holds it (HEVC at 1080p60: 5.0, high tier).
        let constant_quality = ["cq", "qvbr_quality_level"].iter().any(|k| options.contains_key(*k));
        if constant_quality && !options.contains_key("maxrate") {
            options.insert("maxrate".into(), (pixel_rate / 2).max(1).to_string());
        }
        let Some(tools) = tools else {
            return;
        };
        let number =
            |options: &BTreeMap<String, String>, key: &str| options.get(key).and_then(|v| v.parse::<u32>().ok());
        if self.vendor == Vendor::Amd {
            let spare = match tools.amf_throughput {
                [.., 0] => f64::from(tools.engine_fps) * AMF_QUALITY_SHARE * AMF_BFRAMES_SHARE,
                [.., with_bframes] => frames_of_macroblocks(with_bframes),
            };
            let bframes = match number(options, "bf") {
                Some(n) => Some(n),
                None if choose && frames_needed(pixel_rate) <= spare => Some(2),
                None => None,
            }
            .map(|n| n.min(tools.max_bframes));
            match bframes {
                Some(n) if n > 0 => {
                    options.insert("bf".into(), n.to_string());
                }
                _ => {
                    options.remove("bf");
                }
            }
            if !tools.lookahead {
                options.remove("preanalysis");
            }
            return;
        }
        if self.vendor != Vendor::Nvidia {
            return;
        }
        let lookahead = match number(options, "rc-lookahead") {
            _ if !tools.lookahead => 0,
            Some(frames) => frames,
            None if choose => LOOKAHEAD_FRAMES,
            None => 0,
        };
        let bframes = match number(options, "bf") {
            Some(n) => Some(n),
            None if choose => Some(if lookahead > 0 { 4 } else { 2 }),
            None => None,
        }
        .map(|n| n.min(tools.max_bframes));
        if let Some(n) = bframes {
            options.insert("bf".into(), n.to_string());
        }
        if lookahead > 0 {
            // Its depth and the B-frames share NVENC's 31 frames.
            let room = 31 - bframes.unwrap_or(4).min(31);
            options.insert("rc-lookahead".into(), lookahead.min(room).to_string());
        } else {
            options.remove("rc-lookahead");
        }
        let references = self.family != Family::H264 && tools.bframe_ref_middle && bframes.is_none_or(|n| n >= 2);
        if !references {
            options.remove("b_ref_mode");
        }
        if !tools.temporal_aq {
            options.remove("temporal-aq");
        }
        if matches!(self.family, Family::Hevc | Family::Av1) {
            self.fit_speed(options, tools, choose && lookahead > 0, pixel_rate);
        }
    }

    /// NVENC, HEVC and AV1: the deepest look-ahead when the encoder has the time for it, and a frame split between the
    /// encoders only when its preset cannot keep up otherwise.
    ///
    /// Look-ahead level 3 gives the bits to the frames that need them: at the same size the worst 5 % of the frames of
    /// fast games gain 9 to 36 % (SSIMULACRA 2), for 3 % of HEVC's speed and a third of AV1's (RTX 5080), hence
    /// [`DEEP_LOOKAHEAD_SHARE`] of the speed table. It does not run with the split.
    ///
    /// The split leaves a seam across the middle of the picture (the strongest edge of every 4K encode measured, at all
    /// levels), so NVENC's own choice of it (P1 and P2 from 4K) is turned off, and it is forced only when the preset
    /// (the fastest when none keeps up) is too slow.
    fn fit_speed(
        &self,
        options: &mut BTreeMap<String, String>,
        tools: crate::probe::Tools,
        deep_lookahead: bool,
        pixel_rate: u64,
    ) {
        let speeds = nvenc_speeds(self.family, tools.generation);
        let needed = frames_needed(pixel_rate);
        // FFmpeg's default preset is P4; P6 and P7 are not in the table (slower than P5).
        let preset = options.get("preset").map_or("p4", String::as_str).to_owned();
        let speed = speeds.iter().find(|(p, _)| *p == preset).map(|(_, fps)| *fps);
        if tools.engines >= 2 {
            let split = speed.is_some_and(|fps| fps < needed);
            let mode = if split { "forced" } else { "disabled" };
            options.insert("split_encode_mode".into(), mode.into());
            if split {
                // The fastest preset, taken when none kept up, gives way to the best one the split keeps up with; a
                // preset set by hand stays.
                if speeds.last().is_some_and(|(p, _)| *p == preset)
                    && let Some((better, _)) = speeds.iter().find(|(_, fps)| fps * SPLIT_GAIN >= needed)
                {
                    options.insert("preset".into(), (*better).into());
                }
                return;
            }
        }
        let share = if self.family == Family::Av1 { DEEP_LOOKAHEAD_SHARE.1 } else { DEEP_LOOKAHEAD_SHARE.0 };
        if deep_lookahead
            && tools.lookahead_level >= 3
            && !options.contains_key("lookahead_level")
            && speed.is_some_and(|fps| fps * share >= needed)
        {
            options.insert("lookahead_level".into(), "3".into());
        }
    }

    /// AMF's pre-analysis takes only 8-bit pictures: with 10 bits the encoder does not open (AMD's AMF issue 399), so
    /// it is left out.
    pub fn fit_depth(&self, options: &mut BTreeMap<String, String>, depth: u8) {
        if self.vendor == Vendor::Amd && depth > 8 {
            options.remove("preanalysis");
        }
    }

    pub fn supports_format(&self, depth: u8, chroma: Chroma) -> bool {
        self.pixel_formats.iter().any(|p| p.depth == depth && p.chroma == chroma)
    }
}

/// The frames of 1080p a second an encoder must keep up with for `pixel_rate`, with a third to spare (clocks drop under
/// a game's load, the quality presets add a quarter-resolution pass).
fn frames_needed(pixel_rate: u64) -> f64 {
    pixel_rate as f64 / (1920.0 * 1080.0) * 1.3
}

/// Frames of 1080p a second in `macroblocks` (16 × 16) a second.
fn frames_of_macroblocks(macroblocks: u64) -> f64 {
    macroblocks as f64 * 256.0 / (1920.0 * 1080.0)
}

/// The presets of an NVENC encoder that each gain speed, best first, and the frames of 1080p a second one encoder keeps
/// up with at each, with the presets' other tools (look-ahead, quarter-resolution pass, B-frames). Measured on an RTX
/// 5080 (Blackwell), the slower of 1080p and 4K: HEVC is no faster below P3, AV1 at P4 than P5, H.264 below P2. The
/// older generations are scaled by their P5 in the NVENC Application Note (Video Codec SDK 13.0): Turing and Ampere
/// encode at the same pace; AV1 starts with Ada.
fn nvenc_speeds(family: Family, generation: NvencGeneration) -> &'static [(&'static str, f64)] {
    use NvencGeneration::{Ada, Blackwell, Turing};
    match (family, generation) {
        (Family::H264, Turing) => &[("p4", 401.0), ("p3", 504.0), ("p2", 623.0)],
        (Family::H264, Ada) => &[("p4", 430.0), ("p3", 540.0), ("p2", 668.0)],
        (Family::H264, Blackwell) => &[("p4", 483.0), ("p3", 607.0), ("p2", 751.0)],
        (Family::Hevc, Turing) => &[("p5", 242.0), ("p4", 284.0), ("p3", 401.0)],
        (Family::Hevc, Ada) => &[("p5", 299.0), ("p4", 350.0), ("p3", 495.0)],
        (Family::Hevc, Blackwell) => &[("p5", 378.0), ("p4", 443.0), ("p3", 626.0)],
        (Family::Av1, Blackwell) => &[("p5", 363.0), ("p3", 425.0), ("p2", 459.0), ("p1", 544.0)],
        (Family::Av1, _) => &[("p5", 290.0), ("p3", 340.0), ("p2", 367.0), ("p1", 435.0)],
        _ => &[],
    }
}

/// The share of NVENC's speed (HEVC, AV1) left with look-ahead level 3, the lower of 1080p and 4K (RTX 5080: HEVC 0.97
/// and 0.83, AV1 0.65 and 0.61).
const DEEP_LOOKAHEAD_SHARE: (f64, f64) = (0.83, 0.61);

/// The speed the split between two encoders gives (RTX 5080, 4K at P5: HEVC 124 to 207 FPS, AV1 118 to 194).
const SPLIT_GAIN: f64 = 1.6;

/// The share of AMF's balanced speed its quality preset keeps, and of that what is left with B-frames on (see
/// [`Encoder::fit_tools`]).
const AMF_QUALITY_SHARE: f64 = 0.5;
const AMF_BFRAMES_SHARE: f64 = 0.6;

/// The look-ahead of the presets where the GPU has it, in frames: enough for NVENC to pick the B-frames of each group
/// (OBS takes as many); more costs graphics memory, which the game uses.
pub const LOOKAHEAD_FRAMES: u32 = 8;

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
                return bad(format!("`{}`: pixel_formats and containers must not be empty", e.id));
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
                    return bad(format!("`{}`.{}: `rc.` keys are the rate control's", e.id, p.key));
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
