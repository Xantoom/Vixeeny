// SPDX-License-Identifier: GPL-3.0-or-later
//! What the recording tests share. Each test program uses a part of it.
#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use vixeeny_encode::clock::Fps;
use vixeeny_encode::recorder::{OutputContainer, RecordConfig, Split};
use vixeeny_encode::registry::{Chroma, PresetName, Registry};

/// A folder of its own for a test, removed when the test ends, even early or failing.
pub struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("vixeeny-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// A recording of 320×180 at 30 fps, 8-bit 4:2:0 SDR in Matroska, with encoder `id` and its
/// options of `preset`, no audio.
pub fn config(registry: &Registry, id: &str, preset: PresetName) -> RecordConfig {
    let encoder = registry.get(id).unwrap().clone();
    let options = encoder
        .presets
        .get(preset)
        .iter()
        .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
        .collect();
    RecordConfig {
        crop: None,
        encoder,
        options,
        container: OutputContainer::Mkv,
        output_size: (320, 180),
        fps: Fps::whole(30),
        depth: 8,
        chroma: Chroma::C420,
        hdr: false,
        peak_nits: 1000.0,
        sdr_white_nits: 203.0,
        split: Split::Off,
        keyframe_seconds: 1.0,
        queue: 1_000,
        vfr: false,
        audio: Vec::new(),
        gpu: None,
        replay_seconds: None,
        replay_storage: vixeeny_encode::replay::Storage::Ram,
        files: true,
        buffers: Default::default(),
    }
}

/// Names the parts `rec-<n>.<ext>` in `dir`.
pub fn namer(dir: &Path, ext: &str) -> Box<dyn FnMut(u32) -> PathBuf + Send> {
    let dir = dir.to_path_buf();
    let ext = ext.to_owned();
    Box::new(move |part| dir.join(format!("rec-{part}.{ext}")))
}
