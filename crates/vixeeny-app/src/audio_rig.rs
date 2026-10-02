// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio side of a recording: the sources (one capture per distinct source, even when
//! several tracks use it), one mixer per track, and the pump that moves finished blocks to the
//! recorder. Called from the recording thread, a few times per second.

use std::sync::mpsc::{Receiver, Sender, channel};

use vixeeny_audio::{
    AudioChunk, AudioSink, AudioSource, Mixer, SourceEvent, SourceSpec, TrackPlan, WasapiSource,
};
use vixeeny_encode::recorder::Recorder;

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

pub struct Rig {
    sources: Vec<(SourceSpec, WasapiSource)>,
    rx: Receiver<Msg>,
    tracks: Vec<TrackRt>,
    /// Master-clock time of the recording's zero.
    origin: i64,
}

impl Rig {
    /// Starts every source of `plan`. A source that cannot start now is retried by itself.
    pub fn start(plan: &[TrackPlan], origin: i64) -> Self {
        let (tx, rx) = channel();
        let mut sources: Vec<(SourceSpec, WasapiSource)> = Vec::new();
        let mut tracks = Vec::new();
        for track in plan {
            let mut members = Vec::new();
            for member in &track.members {
                let index = match sources.iter().position(|(s, _)| *s == member.source) {
                    Some(i) => i,
                    None => {
                        sources.push((
                            member.source.clone(),
                            WasapiSource::new(member.source.kind.clone()),
                        ));
                        sources.len() - 1
                    }
                };
                members.push(index);
            }
            let volumes: Vec<f32> = track.members.iter().map(|m| m.volume).collect();
            tracks.push(TrackRt {
                mixer: Mixer::new(&volumes, 0),
                members,
            });
        }
        for (i, (spec, source)) in sources.iter_mut().enumerate() {
            let sink = ChannelSink {
                source: i,
                tx: tx.clone(),
            };
            if let Err(e) = source.start(Box::new(sink)) {
                tracing::warn!("audio source `{}` did not start: {e}", spec.name);
            }
        }
        Self {
            sources,
            rx,
            tracks,
            origin,
        }
    }

    /// Moves what arrived into the mixers and the final blocks to the recorder.
    pub fn pump(&mut self, now_ns: i64, recorder: &Recorder) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Chunk(source, mut chunk) => {
                    chunk.time_ns -= self.origin;
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
