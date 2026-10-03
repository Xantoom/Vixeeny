// SPDX-License-Identifier: GPL-3.0-or-later
//! From the profile's `[audio]` section to tracks (plan 5.10): "one source = one track" (default),
//! "mix everything" into one, or the advanced mode where the user names the tracks.

use vixeeny_common::config::Audio;

use crate::spec::SourceSpec;

/// A source in a track, with its volume.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMember {
    pub source: SourceSpec,
    pub volume: f32,
}

/// One audio track of the output file.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackPlan {
    /// The name the track carries in the file.
    pub title: String,
    pub members: Vec<TrackMember>,
    /// Channels of the track: 2, or 6/8 when `assign_channels` found a surround source.
    pub channels: usize,
}

/// Gives each track the layout of its richest source, at most `max` channels (2 keeps every
/// track stereo). `probe` tells the channels a source delivers when allowed up to `max`.
pub fn assign_channels(
    tracks: &mut [TrackPlan],
    max: usize,
    probe: impl Fn(&crate::SourceKind, usize) -> usize,
) {
    for track in tracks {
        track.channels = track
            .members
            .iter()
            .map(|m| probe(&m.source.kind, max))
            .max()
            .map_or(2, |n| crate::track_channels(n, max));
    }
}

/// The tracks of a profile, and the source names that could not be understood.
pub fn plan_tracks(audio: &Audio) -> (Vec<TrackPlan>, Vec<String>) {
    let volume = |s: &SourceSpec| {
        audio
            .volumes
            .get(&s.name)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 4.0)
    };
    let member = |s: SourceSpec| TrackMember {
        volume: volume(&s),
        source: s,
    };
    let mut unknown = Vec::new();
    let parse_all = |texts: &[String], unknown: &mut Vec<String>| {
        let (good, bad) = crate::spec::parse_sources(texts);
        unknown.extend(bad);
        good
    };

    let mut tracks: Vec<TrackPlan> = match audio.routing.as_str() {
        "mix_all" => {
            let sources = parse_all(&audio.sources, &mut unknown);
            if sources.is_empty() {
                Vec::new()
            } else {
                vec![TrackPlan {
                    channels: 2,
                    title: "Mix".into(),
                    members: sources.into_iter().map(member).collect(),
                }]
            }
        }
        "advanced" => audio
            .tracks
            .iter()
            .filter_map(|t| {
                let sources = parse_all(&t.sources, &mut unknown);
                (!sources.is_empty()).then(|| TrackPlan {
                    channels: 2,
                    title: if t.name.trim().is_empty() {
                        sources[0].title()
                    } else {
                        t.name.trim().to_owned()
                    },
                    members: sources.into_iter().map(member).collect(),
                })
            })
            .collect(),
        // "one_track_per_source", and anything unknown falls back to it.
        _ => parse_all(&audio.sources, &mut unknown)
            .into_iter()
            .map(|s| TrackPlan {
                channels: 2,
                title: s.title(),
                members: vec![member(s)],
            })
            .collect(),
    };

    // Two tracks with the same name get a number: players list them by name.
    for i in 1..tracks.len() {
        let base = tracks[i].title.clone();
        let mut n = 2;
        while tracks[..i].iter().any(|t| t.title == tracks[i].title) {
            tracks[i].title = format!("{base} {n}");
            n += 1;
        }
    }
    (tracks, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vixeeny_common::config::AudioTrack;

    fn audio(routing: &str, sources: &[&str]) -> Audio {
        Audio {
            routing: routing.into(),
            sources: sources.iter().map(|s| (*s).to_owned()).collect(),
            ..Audio::default()
        }
    }

    #[test]
    fn one_track_per_source_is_the_default() {
        let (tracks, unknown) = plan_tracks(&audio(
            "one_track_per_source",
            &["mic", "system", "app:Spotify.exe"],
        ));
        let titles: Vec<_> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Micro", "System", "Spotify"]);
        assert!(tracks.iter().all(|t| t.members.len() == 1));
        assert!(unknown.is_empty());
    }

    #[test]
    fn mix_all_gives_a_single_track() {
        let (tracks, _) = plan_tracks(&audio("mix_all", &["mic", "system"]));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].members.len(), 2);
    }

    #[test]
    fn advanced_tracks_are_the_users_with_shared_sources() {
        let mut a = audio("advanced", &[]);
        a.tracks = vec![
            AudioTrack {
                name: "Tout".into(),
                sources: vec!["mic".into(), "system".into(), "app:Spotify.exe".into()],
            },
            AudioTrack {
                name: "Micro seul".into(),
                sources: vec!["mic".into()],
            },
            AudioTrack {
                name: String::new(),
                sources: vec!["app:Spotify.exe".into()],
            },
            AudioTrack {
                name: "vide".into(),
                sources: vec!["nonsense".into()],
            },
        ];
        let (tracks, unknown) = plan_tracks(&a);
        let titles: Vec<_> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Tout", "Micro seul", "Spotify"]);
        assert_eq!(tracks[0].members.len(), 3);
        assert_eq!(unknown, ["nonsense"]);
    }

    #[test]
    fn a_track_takes_the_richest_layout_of_its_sources_up_to_the_limit() {
        use crate::SourceKind;
        let (mut tracks, _) = plan_tracks(&audio("mix_all", &["mic", "system"]));
        let probe = |kind: &SourceKind, max: usize| match kind {
            SourceKind::System => 8.min(max),
            _ => 2,
        };
        assign_channels(&mut tracks, 8, probe);
        assert_eq!(tracks[0].channels, 8);
        assign_channels(&mut tracks, 6, probe);
        assert_eq!(tracks[0].channels, 6);
        assign_channels(&mut tracks, 2, probe);
        assert_eq!(tracks[0].channels, 2);
    }

    #[test]
    fn volumes_apply_and_are_bounded() {
        let mut a = audio("one_track_per_source", &["mic", "system"]);
        a.volumes.insert("mic".into(), 0.5);
        a.volumes.insert("system".into(), 99.0);
        let (tracks, _) = plan_tracks(&a);
        assert_eq!(tracks[0].members[0].volume, 0.5);
        assert_eq!(tracks[1].members[0].volume, 4.0);
    }

    #[test]
    fn same_names_are_numbered_and_unknown_routing_falls_back() {
        let mut a = audio("whatever", &["mic", "mic:Blue Yeti"]);
        a.volumes.clear();
        let (tracks, _) = plan_tracks(&a);
        let titles: Vec<_> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Micro", "Micro 2"]);
    }
}
