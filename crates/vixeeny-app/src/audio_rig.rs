// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio side of a recording: the sources (one capture per distinct source, even when
//! several tracks use it), one mixer per track, and the pump that moves finished blocks to the
//! recorder. Called from the recording thread, a few times per second.

use std::sync::mpsc::{Receiver, Sender, channel};

use vixeeny_audio::WasapiSource as OsSource;
use vixeeny_audio::{
    AudioChunk, AudioSink, AudioSource, Mixer, SourceEvent, SourceSpec, TrackPlan,
};
use vixeeny_common::config::Config;
use vixeeny_encode::recorder::Recorder;

use crate::toast::{Failed, Toast};

enum Msg {
    Chunk(usize, AudioChunk),
    Event(usize, SourceEvent),
}

struct ChannelSink {
    source: usize,
    tx: Sender<Msg>,
}

impl AudioSink for ChannelSink {
    fn on_audio(&mut self, chunk: AudioChunk) {
        let _ = self.tx.send(Msg::Chunk(self.source, chunk));
    }

    fn on_event(&mut self, event: SourceEvent) {
        let _ = self.tx.send(Msg::Event(self.source, event));
    }
}

struct TrackRt {
    mixer: Mixer,
    /// Source index of each member, in the order of the mixer's volumes.
    members: Vec<usize>,
}

/// Noise reduction of one microphone source. The filter holds a little audio inside, so output
/// samples are placed on the timeline by counting them from where the input stream began.
struct Denoise {
    filter: vixeeny_encode::denoise::Denoiser,
    /// Time of the first sample of the current continuous stretch, nanoseconds.
    base_ns: i64,
    in_frames: i64,
    out_frames: i64,
}

/// A hole in the input longer than this restarts the filter (the source was lost and came back).
const DENOISE_GAP_NS: i64 = 50_000_000;

fn frames_ns(frames: i64) -> i64 {
    frames * 1_000_000_000 / 48_000
}

impl Denoise {
    fn new() -> Option<Self> {
        match vixeeny_encode::denoise::Denoiser::new(12.0, -45.0) {
            Ok(filter) => Some(Self {
                filter,
                base_ns: 0,
                in_frames: 0,
                out_frames: 0,
            }),
            Err(e) => {
                tracing::warn!("microphone noise reduction is not available: {e}");
                None
            }
        }
    }

    fn apply(&mut self, chunk: AudioChunk) -> Option<AudioChunk> {
        if chunk.channels != 2 {
            return Some(chunk);
        }
        let expected = self.base_ns + frames_ns(self.in_frames);
        if self.in_frames == 0 || (chunk.time_ns - expected).abs() > DENOISE_GAP_NS {
            if let Some(fresh) = Self::new() {
                *self = fresh;
            }
            self.base_ns = chunk.time_ns;
        }
        self.in_frames += (chunk.samples.len() / 2) as i64;
        let samples = self.filter.process(&chunk.samples).ok()?;
        if samples.is_empty() {
            return None;
        }
        let time_ns = self.base_ns + frames_ns(self.out_frames);
        self.out_frames += (samples.len() / 2) as i64;
        Some(AudioChunk::stereo(time_ns, samples))
    }
}

pub struct Rig {
    denoise: Vec<Option<Denoise>>,
    sources: Vec<(SourceSpec, OsSource)>,
    rx: Receiver<Msg>,
    tracks: Vec<TrackRt>,
    /// Master-clock time of the recording's zero.
    origin: i64,
    /// The settings, for the notification when a source is lost.
    notice: Config,
}

impl Rig {
    /// Starts every source of `plan`. A source that cannot start now is retried by itself.
    pub fn start(plan: &[TrackPlan], origin: i64, notice: Config, mic_denoise: bool) -> Self {
        let (tx, rx) = channel();
        let mut wanted: Vec<(SourceSpec, Vec<usize>)> = Vec::new();
        let mut tracks = Vec::new();
        for track in plan {
            let mut members = Vec::new();
            for member in &track.members {
                let index = match wanted.iter().position(|(s, _)| *s == member.source) {
                    Some(i) => i,
                    None => {
                        wanted.push((member.source.clone(), Vec::new()));
                        wanted.len() - 1
                    }
                };
                // A source shared by several tracks serves the widest of them.
                wanted[index].1.push(track.channels);
                members.push(index);
            }
            let volumes: Vec<f32> = track.members.iter().map(|m| m.volume).collect();
            tracks.push(TrackRt {
                mixer: Mixer::with_channels(&volumes, 0, track.channels),
                members,
            });
        }
        let mut sources: Vec<(SourceSpec, OsSource)> = wanted
            .into_iter()
            .map(|(spec, channels)| {
                let widest = channels.into_iter().max().unwrap_or(2);
                let source = OsSource::with_max_channels(spec.kind.clone(), widest);
                (spec, source)
            })
            .collect();
        for (i, (spec, source)) in sources.iter_mut().enumerate() {
            let sink = ChannelSink {
                source: i,
                tx: tx.clone(),
            };
            if let Err(e) = source.start(Box::new(sink)) {
                tracing::warn!("audio source `{}` did not start: {e}", spec.name);
            }
        }
        let denoise = sources
            .iter()
            .map(|(spec, _)| {
                (mic_denoise && matches!(spec.kind, vixeeny_audio::SourceKind::Microphone(_)))
                    .then(Denoise::new)
                    .flatten()
            })
            .collect();
        Self {
            denoise,
            sources,
            rx,
            tracks,
            origin,
            notice,
        }
    }

    /// Moves what arrived into the mixers and the final blocks to the recorder.
    pub fn pump(&mut self, now_ns: i64, recorder: &Recorder) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Chunk(source, mut chunk) => {
                    chunk.time_ns -= self.origin;
                    if let Some(Some(filter)) = self.denoise.get_mut(source) {
                        match filter.apply(chunk) {
                            Some(filtered) => chunk = filtered,
                            None => continue,
                        }
                    }
                    for track in &mut self.tracks {
                        for (pos, member) in track.members.iter().enumerate() {
                            if *member == source {
                                track.mixer.push(pos, &chunk);
                            }
                        }
                    }
                }
                Msg::Event(source, SourceEvent::Lost(reason)) => {
                    // The track stays silent until the source returns (CA-AUD-2, CA-AUD-3).
                    let name = self
                        .sources
                        .get(source)
                        .map_or("?", |(s, _)| s.name.as_str());
                    tracing::warn!("audio source `{name}` lost: {reason}");
                    let text = format!("{name}: {reason}");
                    crate::toast::notify(&self.notice, &Toast::Failed(Failed::Audio, text));
                }
                Msg::Event(source, SourceEvent::Back) => {
                    let name = self
                        .sources
                        .get(source)
                        .map_or("?", |(s, _)| s.name.as_str());
                    tracing::info!("audio source `{name}` is back");
                }
            }
        }
        for (i, track) in self.tracks.iter_mut().enumerate() {
            for block in track.mixer.drain(now_ns) {
                recorder.push_audio(i, block.samples);
            }
        }
    }

    pub fn pause(&mut self, now_ns: i64) {
        for track in &mut self.tracks {
            track.mixer.pause(now_ns);
        }
    }

    pub fn resume(&mut self, now_ns: i64, recorder: &Recorder) {
        for (i, track) in self.tracks.iter_mut().enumerate() {
            for block in track.mixer.resume(now_ns) {
                recorder.push_audio(i, block.samples);
            }
        }
    }

    /// Stops the sources and sends what is left.
    pub fn finish(&mut self, now_ns: i64, recorder: &Recorder) {
        for (_, source) in &mut self.sources {
            let _ = source.stop();
        }
        self.pump(now_ns, recorder);
        for (i, track) in self.tracks.iter_mut().enumerate() {
            for block in track.mixer.finish(now_ns) {
                recorder.push_audio(i, block.samples);
            }
        }
    }
}
