// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-convert — batch image conversion (plan 5.5). No UI, no OS: a job turns files and
//! folders into another format on every core, reports progress and can be cancelled.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use vixeeny_image::{ImageFormat, Settings, SourceFormat};

/// What to do when the output file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnExisting {
    /// `name (2).ext`, `name (3).ext`…
    #[default]
    Rename,
    Overwrite,
    Skip,
}

impl OnExisting {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "rename" => Some(Self::Rename),
            "overwrite" => Some(Self::Overwrite),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub inputs: Vec<PathBuf>,
    pub format: ImageFormat,
    pub settings: Settings,
    /// `None`: next to each input.
    pub output_dir: Option<PathBuf>,
    pub on_existing: OnExisting,
    /// Worker threads; 0 = one per core.
    pub threads: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Converted(PathBuf),
    /// The output exists and the job says to skip, or the input is already in the wanted format
    /// at its destination.
    Skipped(PathBuf),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub input: PathBuf,
    pub outcome: Outcome,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Files never started because of a cancellation.
    pub cancelled: usize,
}

/// What the conversion window lets the user pick, in format-independent terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choices {
    pub format: ImageFormat,
    /// 1 to 100; ignored by PNG and by the lossless modes.
    pub quality: u8,
    /// WebP and JPEG XL only.
    pub lossless: bool,
}

impl Choices {
    /// The starting point shown for `format`, from the configured settings.
    pub fn from_settings(format: ImageFormat, settings: &Settings) -> Self {
        let (quality, lossless) = match format {
            ImageFormat::Png => (100, true),
            ImageFormat::Jpeg => (settings.jpeg.quality, false),
            ImageFormat::WebP => (settings.webp.quality.round() as u8, settings.webp.lossless),
            ImageFormat::Avif => (settings.avif.quality, false),
            ImageFormat::Jxl => (
                (100.0 - settings.jxl.distance * 10.0).clamp(1.0, 100.0) as u8,
                settings.jxl.lossless,
            ),
        };
        Self {
            format,
            quality: quality.clamp(1, 100),
            lossless,
        }
    }

    pub const fn has_quality(&self) -> bool {
        match self.format {
            ImageFormat::Png => false,
            ImageFormat::Jpeg | ImageFormat::Avif => true,
            ImageFormat::WebP | ImageFormat::Jxl => !self.lossless,
        }
    }

    pub const fn has_lossless(&self) -> bool {
        matches!(self.format, ImageFormat::WebP | ImageFormat::Jxl)
    }

    /// `base` with the choices applied.
    pub fn apply(&self, mut base: Settings) -> Settings {
        let q = self.quality.clamp(1, 100);
        match self.format {
            ImageFormat::Png => {}
            ImageFormat::Jpeg => base.jpeg.quality = q,
            ImageFormat::WebP => {
                base.webp.lossless = self.lossless;
                base.webp.quality = f32::from(q);
            }
            ImageFormat::Avif => base.avif.quality = q,
            ImageFormat::Jxl => {
                base.jxl.lossless = self.lossless;
                base.jxl.distance = ((100.0 - f32::from(q)) / 10.0).max(0.1);
            }
        }
        base
    }
}

/// Files to convert: the readable files among `inputs`, and the ones inside the folders
/// (recursively), in a stable order.
pub fn collect(inputs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else if is_supported(&path) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for input in inputs {
        if input.is_dir() {
            walk(input, &mut out);
        } else if is_supported(input) {
            out.push(input.clone());
        }
    }
    out.dedup();
    out
}

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(SourceFormat::from_extension)
        .is_some()
}

/// Where the output of `input` goes, before any collision handling.
pub fn output_path(input: &Path, format: ImageFormat, output_dir: Option<&Path>) -> PathBuf {
    let dir = output_dir
        .or_else(|| input.parent())
        .unwrap_or(Path::new("."));
    let stem = input
        .file_stem()
        .map_or_else(|| "image".into(), |s| s.to_string_lossy());
    dir.join(format!("{stem}.{}", format.extension()))
}

fn numbered(path: &Path, n: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .map_or_else(String::new, |e| format!(".{}", e.to_string_lossy()));
    path.with_file_name(format!("{stem} ({n}){ext}"))
}

/// Writes `bytes` at `wanted`, following `on_existing`. The file is claimed with `create_new`,
/// so parallel workers (and a batch whose inputs share a name) never overwrite each other, and
/// a source file is never replaced by its own conversion.
fn store(
    wanted: &Path,
    bytes: &[u8],
    on_existing: OnExisting,
    source: &Path,
) -> std::io::Result<Outcome> {
    if let Some(dir) = wanted.parent() {
        fs::create_dir_all(dir)?;
    }
    let same_as_source =
        fs::canonicalize(wanted).ok() == fs::canonicalize(source).ok() && wanted.exists();
    let mut candidate = wanted.to_path_buf();
    let mut n = 1;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                return match file.write_all(bytes).and_then(|()| file.flush()) {
                    Ok(()) => Ok(Outcome::Converted(candidate)),
                    Err(e) => {
                        drop(file);
                        let _ = fs::remove_file(&candidate);
                        Err(e)
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match on_existing {
                OnExisting::Skip => return Ok(Outcome::Skipped(candidate)),
                OnExisting::Overwrite if !same_as_source && n == 1 => {
                    let tmp = candidate.with_extension("vixeeny-tmp");
                    fs::write(&tmp, bytes)?;
                    if let Err(e) = fs::rename(&tmp, &candidate) {
                        let _ = fs::remove_file(&tmp);
                        return Err(e);
                    }
                    return Ok(Outcome::Converted(candidate));
                }
                _ => {
                    n += 1;
                    candidate = numbered(wanted, n);
                }
            },
            Err(e) => return Err(e),
        }
    }
}

fn convert_one(input: &Path, job: &Job) -> Outcome {
    let result = (|| -> Result<Outcome, String> {
        let bytes = fs::read(input).map_err(|e| e.to_string())?;
        let picture = vixeeny_image::decode(&bytes).map_err(|e| e.to_string())?;
        drop(bytes);
        let encoded = vixeeny_image::encode(job.format, &picture.as_bgra(), &job.settings)
            .map_err(|e| e.to_string())?;
        let wanted = output_path(input, job.format, job.output_dir.as_deref());
        store(&wanted, &encoded, job.on_existing, input).map_err(|e| e.to_string())
    })();
    result.unwrap_or_else(Outcome::Failed)
}

/// Converts `files` (see [`collect`]). `on_event` is called after each file, from worker
/// threads. When `cancel` becomes true, running conversions finish and no new one starts.
pub fn run(
    files: &[PathBuf],
    job: &Job,
    cancel: &AtomicBool,
    on_event: &(dyn Fn(Event) + Sync),
) -> Summary {
    let total = files.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let tally = std::sync::Mutex::new(Summary::default());
    let threads = if job.threads == 0 {
        std::thread::available_parallelism().map_or(2, usize::from)
    } else {
        job.threads
    }
    .clamp(1, total.max(1));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(input) = files.get(i) else { return };
                    let outcome = convert_one(input, job);
                    if let Ok(mut t) = tally.lock() {
                        match &outcome {
                            Outcome::Converted(_) => t.converted += 1,
                            Outcome::Skipped(_) => t.skipped += 1,
                            Outcome::Failed(_) => t.failed += 1,
                        }
                    }
                    let done = done.fetch_add(1, Ordering::Relaxed) + 1;
                    on_event(Event {
                        input: input.clone(),
                        outcome,
                        done,
                        total,
                    });
                }
            });
        }
    });
    let mut summary = tally.into_inner().unwrap_or_default();
    summary.cancelled = total - summary.converted - summary.skipped - summary.failed;
    summary
}

#[cfg(test)]
mod tests;
