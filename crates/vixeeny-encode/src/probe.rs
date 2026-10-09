// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware probing (plan 6.3). The driver of every GPU is asked what its video encoder can do
//! (codecs, 10-bit, 4:2:2 / 4:4:4, largest picture): nothing is encoded, so the GPU is left
//! alone. Only a driver that cannot answer (a runtime older than the query) gets trial sessions
//! instead: one per declared pixel format (256×256, then 3840×2160, with and without HDR).
//! Software encoders always get a trial session (it only takes the CPU). The work is done by a
//! [`Prober`], so it can be tested with fake adapters; the real one is `ffmpeg_probe`.
//!
//! The probe runs in a child process (`vixeeny-app --probe`, see [`run_child`]) so that a driver
//! crash cannot take the app down; its result is cached in `hw_cache.toml`.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::registry::{Chroma, Encoder, Family, Kind, PixelFormatSpec, Registry, Vendor};

pub const SMALL: (u32, u32) = (256, 256);
pub const UHD: (u32, u32) = (3840, 2160);

/// A GPU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adapter {
    /// Position among the adapters of the same vendor (what `gpu=` style options expect).
    pub index: u32,
    pub name: String,
    pub vendor: Vendor,
    pub vendor_id: u32,
    pub device_id: u32,
    pub driver_version: String,
    /// A software renderer (WARP, llvmpipe…): never used for encoding.
    pub software: bool,
}

/// PCI vendor ids.
pub fn vendor_from_pci(id: u32) -> Vendor {
    match id {
        0x10DE => Vendor::Nvidia,
        0x1002 | 0x1022 => Vendor::Amd,
        0x8086 => Vendor::Intel,
        _ => Vendor::None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatProbe {
    pub depth: u8,
    pub chroma: Chroma,
    /// Opens at 3840×2160 too.
    pub uhd: bool,
    /// Opens with HDR10 signalling (BT.2020 PQ).
    pub hdr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncoderProbe {
    pub id: String,
    /// The GPU it runs on (`None` for software encoders).
    pub adapter: Option<u32>,
    pub formats: Vec<FormatProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProbeResult {
    /// What the result is valid for: see [`cache_key`].
    pub key: String,
    pub adapters: Vec<Adapter>,
    pub encoders: Vec<EncoderProbe>,
}

impl ProbeResult {
    /// Whether `encoder` opened with this format on some GPU.
    pub fn supports(&self, encoder: &str, depth: u8, chroma: Chroma, hdr: bool) -> bool {
        self.encoders.iter().filter(|e| e.id == encoder).any(|e| {
            e.formats
                .iter()
                .any(|f| f.depth == depth && f.chroma == chroma && (!hdr || f.hdr))
        })
    }

    pub fn is_available(&self, encoder: &str) -> bool {
        self.encoders
            .iter()
            .any(|e| e.id == encoder && !e.formats.is_empty())
    }
}

/// What a GPU's encoder can do for one codec, as its driver describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverCaps {
    pub family: Family,
    pub ten_bit: bool,
    /// The chroma subsamplings it accepts (4:2:0 always).
    pub chroma: Vec<Chroma>,
    /// The largest picture (width, height).
    pub max: (u32, u32),
}

impl DriverCaps {
    fn accepts(&self, format: &PixelFormatSpec) -> bool {
        (format.depth <= 8 || self.ten_bit) && self.chroma.contains(&format.chroma)
    }
}

/// What the probe needs from the OS and FFmpeg.
pub trait Prober {
    fn adapters(&self) -> Vec<Adapter>;
    /// Whether FFmpeg has `encoder` built in.
    fn built_in(&self, encoder: &Encoder) -> bool;
    /// The encoders of `adapter`'s GPU as its driver describes them, one entry per codec. `Err`
    /// when the driver cannot be asked: the probe then opens trial sessions.
    fn driver_caps(&self, adapter: &Adapter) -> Result<Vec<DriverCaps>, String>;
    /// Opens a trial session and encodes one frame. `adapter` is the GPU to use, if any.
    fn try_open(
        &self,
        encoder: &Encoder,
        adapter: Option<&Adapter>,
        format: &PixelFormatSpec,
        size: (u32, u32),
        hdr: bool,
    ) -> Result<(), String>;
}

/// The GPUs an encoder can run on.
fn candidate_adapters<'a>(encoder: &Encoder, adapters: &'a [Adapter]) -> Vec<Option<&'a Adapter>> {
    match encoder.vendor {
        Vendor::None => vec![None],
        Vendor::Nvidia | Vendor::Amd | Vendor::Intel => adapters
            .iter()
            .filter(|a| !a.software && a.vendor == encoder.vendor)
            .map(Some)
            .collect(),
    }
}

/// The formats `caps` accepts, among those the registry declares.
fn described_formats(encoder: &Encoder, caps: &[DriverCaps]) -> Vec<FormatProbe> {
    let Some(caps) = caps.iter().find(|c| c.family == encoder.family) else {
        return Vec::new();
    };
    let uhd = caps.max.0 >= UHD.0 && caps.max.1 >= UHD.1;
    encoder
        .pixel_formats
        .iter()
        .filter(|f| caps.accepts(f))
        .map(|f| FormatProbe {
            depth: f.depth,
            chroma: f.chroma,
            uhd,
            // HDR10 is signalling on a 10-bit stream.
            hdr: encoder.hdr && f.depth >= 10,
        })
        .collect()
}

/// Trial sessions, for software encoders and drivers that cannot describe themselves.
fn tried_formats(
    encoder: &Encoder,
    adapter: Option<&Adapter>,
    prober: &dyn Prober,
) -> Vec<FormatProbe> {
    let hardware = encoder.kind == Kind::Hardware;
    let mut out = Vec::new();
    for format in &encoder.pixel_formats {
        if prober
            .try_open(encoder, adapter, format, SMALL, false)
            .is_err()
        {
            continue;
        }
        // Software encoders are not tried at 4K (slow, and size-independent).
        let uhd = !hardware
            || prober
                .try_open(encoder, adapter, format, UHD, false)
                .is_ok();
        let hdr = encoder.hdr
            && format.depth >= 10
            && prober
                .try_open(
                    encoder,
                    adapter,
                    format,
                    if uhd { UHD } else { SMALL },
                    true,
                )
                .is_ok();
        out.push(FormatProbe {
            depth: format.depth,
            chroma: format.chroma,
            uhd,
            hdr,
        });
    }
    out
}

/// What the answer of the probe depends on besides the machine: the way it tries the encoders
/// (bump [`PROBE_REVISION`] when that changes), the encoders it tries (the registry) and the
/// FFmpeg it tries them with. A new version of Vixeeny that changes none of them keeps the
/// cache: the probe (a few seconds of encoder sessions on the GPU) does not run again.
pub fn identity() -> String {
    // FNV-1a: stable from one build to the next, unlike the standard hasher.
    let registry = crate::registry::REGISTRY
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    // SAFETY: a plain query of the linked library.
    let avcodec = unsafe { ffmpeg_next::ffi::avcodec_version() };
    format!("probe {PROBE_REVISION}, registry {registry:016x}, avcodec {avcodec}")
}

/// The way the probe works: bumped when a change of it must run it again everywhere.
pub const PROBE_REVISION: u32 = 2;

/// Probes every encoder. `version` is what the answer depends on besides the machine (see
/// [`identity`]), part of the cache key.
pub fn probe(registry: &Registry, prober: &dyn Prober, version: &str) -> ProbeResult {
    let adapters = prober.adapters();
    // One question per GPU, whatever the number of its encoders.
    let described: Vec<Result<Vec<DriverCaps>, String>> = adapters
        .iter()
        .map(|a| {
            if a.software || a.vendor == Vendor::None {
                Ok(Vec::new())
            } else {
                prober.driver_caps(a).inspect_err(|e| {
                    tracing::warn!("{}: the driver cannot describe its encoders ({e})", a.name);
                })
            }
        })
        .collect();
    let mut encoders = Vec::new();
    for encoder in registry.encoders() {
        if !prober.built_in(encoder) {
            continue;
        }
        for adapter in candidate_adapters(encoder, &adapters) {
            let caps = adapter
                .and_then(|a| adapters.iter().position(|b| b == a))
                .map(|i| &described[i]);
            let formats = match caps {
                Some(Ok(caps)) => described_formats(encoder, caps),
                _ => tried_formats(encoder, adapter, prober),
            };
            if !formats.is_empty() {
                encoders.push(EncoderProbe {
                    id: encoder.id.clone(),
                    adapter: adapter.map(|a| a.index),
                    formats,
                });
            }
        }
    }
    ProbeResult {
        key: cache_key(&adapters, version),
        adapters,
        encoders,
    }
}

/// The set {GPU, driver version, [`identity`]}: a new key means a new probe.
pub fn cache_key(adapters: &[Adapter], version: &str) -> String {
    let mut parts: Vec<String> = adapters
        .iter()
        .map(|a| {
            format!(
                "{:04x}:{:04x}:{}:{}",
                a.vendor_id, a.device_id, a.name, a.driver_version
            )
        })
        .collect();
    parts.sort();
    format!("vixeeny {version}; {}", parts.join("; "))
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("cannot start the probe: {0}")]
    Spawn(std::io::Error),
    #[error("the probe did not finish in time")]
    Timeout,
    #[error("the probe process failed ({0})")]
    Failed(String),
    #[error("unreadable probe output: {0}")]
    Output(String),
    #[error("cache: {0}")]
    Cache(String),
}

/// What the probe process prints.
pub fn to_toml(result: &ProbeResult) -> Result<String, ProbeError> {
    toml::to_string(result).map_err(|e| ProbeError::Output(e.to_string()))
}

pub fn from_toml(text: &str) -> Result<ProbeResult, ProbeError> {
    toml::from_str(text).map_err(|e| ProbeError::Output(e.to_string()))
}

/// Runs `<exe> --probe` and reads its output; the child is killed after `timeout`.
pub fn run_child(exe: &Path, timeout: Duration) -> Result<ProbeResult, ProbeError> {
    let mut child = Command::new(exe)
        .arg("--probe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(ProbeError::Spawn)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProbeError::Failed("no stdout".into()))?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(ProbeError::Spawn)? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProbeError::Timeout);
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let text = reader
        .join()
        .map_err(|_| ProbeError::Failed("reader panicked".into()))?
        .map_err(|e| ProbeError::Output(e.to_string()))?;
    if !status.success() {
        return Err(ProbeError::Failed(status.to_string()));
    }
    from_toml(&text)
}

/// The cached result, when it was made for exactly `key`.
pub fn load_cache(path: &Path, key: &str) -> Option<ProbeResult> {
    let text = std::fs::read_to_string(path).ok()?;
    from_toml(&text).ok().filter(|r| r.key == key)
}

pub fn save_cache(path: &Path, result: &ProbeResult) -> Result<(), ProbeError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ProbeError::Cache(e.to_string()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, to_toml(result)?).map_err(|e| ProbeError::Cache(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| ProbeError::Cache(e.to_string()))
}

/// The cache when it is current, else a fresh probe (`run`) which is then cached. `adapters`
/// are the GPUs present now.
pub fn cached_or_probe(
    path: &Path,
    adapters: &[Adapter],
    version: &str,
    run: impl FnOnce() -> Result<ProbeResult, ProbeError>,
) -> Result<ProbeResult, ProbeError> {
    let key = cache_key(adapters, version);
    if let Some(hit) = load_cache(path, &key) {
        return Ok(hit);
    }
    let fresh = run()?;
    // A failed write must not lose the result.
    let _ = save_cache(path, &fresh);
    Ok(fresh)
}
