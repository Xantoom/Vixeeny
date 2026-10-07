// SPDX-License-Identifier: GPL-3.0-or-later
//! The rows of each page of the settings window.

use vixeeny_common::config::Config;
use vixeeny_common::i18n::Key;
use vixeeny_encode::registry::ParamType;

use crate::encoders::{self, param_label, param_range};
use crate::{
    Current, Env, Invalid, Kind, Opt, Row, Value, choice, folder, header, hinted, info, number,
    opt, row, segmented, slider, text, toggle, when, when_boxed,
};

/// `off`, `size:<MB>` or `duration:<minutes>` → (kind, amount).
fn split_parts(mode: &str) -> (&str, i64) {
    match mode.split_once(':') {
        Some((kind @ ("size" | "duration"), n)) => (kind, n.parse().unwrap_or(0)),
        _ => ("off", 0),
    }
}

fn split_join(kind: &str, amount: i64) -> String {
    match kind {
        "size" | "duration" => format!("{kind}:{}", amount.max(1)),
        _ => "off".into(),
    }
}

/// The frame rates a recording can have (those above the fastest monitor are not offered).
const FRAME_RATES: [u32; 8] = [24, 30, 60, 90, 120, 144, 165, 240];

/// The offered frame rate closest to `fps` (a setting from another machine or an older list).
fn fps_option(fps: u32, offered: &[u32]) -> u32 {
    offered
        .iter()
        .copied()
        .min_by_key(|f| f.abs_diff(fps))
        .unwrap_or(60)
}

fn hdr_on(setting: &str) -> bool {
    matches!(setting, "keep_hdr" | "hdr")
}

pub fn general(env: &Env) -> Vec<Row> {
    let t = |k| env.t(k);
    vec![
        choice(
            "language",
            t(Key::SetLanguage),
            vec![
                opt("auto", t(Key::SetLangAuto)),
                opt("fr", "Français"),
                opt("en", "English"),
            ],
            |c| c.general.language.clone(),
            |c, v| c.general.language = v,
        ),
        segmented(
            "theme",
            t(Key::SetTheme),
            vec![
                opt("system", t(Key::SetThemeSystem)),
                opt("light", t(Key::SetThemeLight)),
                opt("dark", t(Key::SetThemeDark)),
            ],
            |c| c.general.theme.clone(),
            |c, v| c.general.theme = v,
        ),
        toggle(
            "autostart",
            t(Key::SetAutostart),
            |c| c.general.autostart,
            |c, v| c.general.autostart = v,
        ),
        toggle(
            "notifications",
            t(Key::SetNotifications),
            |c| c.general.notifications,
            |c, v| c.general.notifications = v,
        ),
        toggle(
            "sounds",
            t(Key::SetSounds),
            |c| c.general.sounds,
            |c, v| c.general.sounds = v,
        ),
        header("h_naming"),
        hinted(
            text(
                "template",
                t(Key::SetTemplate),
                |c| c.paths.filename_template.clone(),
                |c, v| {
                    if !v.is_empty() {
                        c.paths.filename_template = v;
                    }
                },
            ),
            t(Key::SetTemplateHint),
        ),
        toggle(
            "foreground_app",
            t(Key::SetForegroundApp),
            |c| c.paths.use_foreground_app,
            |c, v| c.paths.use_foreground_app = v,
        ),
    ]
}

/// The side strip and the recording widget.
pub fn overlay(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let mut rows = vec![
        choice(
            "overlay_edge",
            t(Key::SetOverlayEdge),
            vec![
                opt("left", t(Key::SetEdgeLeft)),
                opt("right", t(Key::SetEdgeRight)),
                opt("top", t(Key::SetEdgeTop)),
                opt("bottom", t(Key::SetEdgeBottom)),
            ],
            |c| c.overlay.edge.clone(),
            |c, v| c.overlay.edge = v,
        ),
        header("h_widget"),
        toggle(
            "widget",
            t(Key::SetWidget),
            |c| c.recording_widget.enabled,
            |c, v| c.recording_widget.enabled = v,
        ),
    ];
    if config.recording_widget.enabled {
        rows.push(choice(
            "widget_corner",
            t(Key::SetWidgetCorner),
            vec![
                opt("top_left", t(Key::SetCornerTl)),
                opt("top_right", t(Key::SetCornerTr)),
                opt("bottom_left", t(Key::SetCornerBl)),
                opt("bottom_right", t(Key::SetCornerBr)),
            ],
            |c| c.recording_widget.corner.clone(),
            |c, v| c.recording_widget.corner = v,
        ));
        rows.push(toggle(
            "widget_hide",
            t(Key::SetWidgetHide),
            |c| c.recording_widget.auto_hide,
            |c, v| c.recording_widget.auto_hide = v,
        ));
    }
    rows
}

/// Screenshots: where they go, their format, what they show.
pub fn image(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let mut rows = vec![
        folder(
            "dir_images",
            t(Key::SetFolder),
            |c| c.paths.images.clone(),
            |c, v| c.paths.images = v,
        ),
        toggle(
            "sub_images",
            t(Key::SetSubfolder),
            |c| c.paths.per_app_subfolder.images,
            |c, v| c.paths.per_app_subfolder.images = v,
        ),
        header("h_format"),
        segmented(
            "image_format",
            t(Key::SetImageFormat),
            vec![
                opt("png", "PNG"),
                opt("jpeg", "JPEG"),
                opt("webp", "WebP"),
                opt("avif", "AVIF"),
                opt("jxl", "JPEG XL"),
            ],
            |c| c.image.format.clone(),
            |c, v| c.image.format = v,
        ),
    ];
    match config.image.format.as_str() {
        "jpeg" => {
            rows.push(slider(
                "jpeg_quality",
                t(Key::SetJpegQuality),
                (1, 100, 1),
                |c| i64::from(c.image.jpeg.quality),
                |c, v| c.image.jpeg.quality = v as u8,
            ));
            rows.push(segmented(
                "jpeg_chroma",
                t(Key::SetJpegChroma),
                vec![
                    opt("444", t(Key::OptChromaSharp)),
                    opt("420", t(Key::OptChromaLight)),
                ],
                |c| c.image.jpeg.chroma.clone(),
                |c, v| c.image.jpeg.chroma = v,
            ));
        }
        "avif" => {
            rows.push(slider(
                "avif_quality",
                t(Key::SetAvifQuality),
                (0, 100, 1),
                |c| i64::from(c.image.avif.quality),
                |c, v| c.image.avif.quality = v as u8,
            ));
            rows.push(segmented(
                "avif_depth",
                t(Key::SetAvifDepth),
                vec![opt("8", "8 bits"), opt("10", "10 bits")],
                |c| c.image.avif.depth.to_string(),
                |c, v| c.image.avif.depth = v.parse().unwrap_or(10),
            ));
        }
        _ => {}
    }
    rows.extend([
        header("h_capture"),
        toggle(
            "clipboard",
            t(Key::SetCopyClipboard),
            |c| c.image.copy_to_clipboard,
            |c, v| c.image.copy_to_clipboard = v,
        ),
        toggle(
            "image_cursor",
            t(Key::SetShowCursor),
            |c| c.image.show_cursor,
            |c, v| c.image.show_cursor = v,
        ),
    ]);
    // Only when Windows shows HDR: there is nothing to keep otherwise.
    if env.display.hdr {
        rows.push(toggle(
            "image_hdr",
            t(Key::SetHdrEnable),
            |c| matches!(c.image.hdr.as_str(), "keep_hdr" | "hdr"),
            |c, v| c.image.hdr = if v { "keep_hdr" } else { "tonemap_sdr" }.into(),
        ));
    }
    rows
}

/// The registry parameters of the encoder in use, as editable rows (the `custom` preset).
fn custom_rows(env: &Env, encoder: &str) -> Vec<Row> {
    let Some(spec) = encoders::spec(encoder) else {
        return Vec::new();
    };
    let lang = env.lang;
    spec.params
        .iter()
        .map(|param| {
            let key = param.key.clone();
            let default = param.default.to_ffmpeg();
            let id = format!("p:{key}");
            let label = param_label(&key, lang);
            let read = {
                let (key, default) = (key.clone(), default.clone());
                move |c: &Config| c.cur().params.get(&key).cloned().unwrap_or(default.clone())
            };
            let write = {
                let key = key.clone();
                move |c: &mut Config, v: String| {
                    c.cur_mut().params.insert(key.clone(), v);
                }
            };
            let text_row = |kind: Kind| {
                let (read, write) = (read.clone(), write.clone());
                row(
                    id.clone(),
                    label.clone(),
                    kind,
                    Box::new(move |c| Value::Text(read(c))),
                    Box::new(move |c, v| match v {
                        Value::Text(t) => {
                            write(c, t);
                            Ok(())
                        }
                        _ => Err(Invalid),
                    }),
                )
            };
            let number_row = |kind: Kind| {
                let (read, write) = (read.clone(), write.clone());
                row(
                    id.clone(),
                    label.clone(),
                    kind,
                    Box::new(move |c| Value::Int(read(c).parse::<f64>().map_or(0, |n| n as i64))),
                    Box::new(move |c, v| match v {
                        Value::Int(n) => {
                            write(c, n.to_string());
                            Ok(())
                        }
                        _ => Err(Invalid),
                    }),
                )
            };
            match param.kind {
                ParamType::Enum => {
                    let options = param.values.iter().map(|v| opt(v, v.as_str())).collect();
                    text_row(Kind::Choice(options))
                }
                ParamType::Bool => row(
                    id.clone(),
                    label.clone(),
                    Kind::Toggle,
                    {
                        let read = read.clone();
                        Box::new(move |c| Value::Bool(matches!(read(c).as_str(), "1" | "true")))
                    },
                    {
                        let write = write.clone();
                        Box::new(move |c, v| match v {
                            Value::Bool(b) => {
                                write(c, u8::from(b).to_string());
                                Ok(())
                            }
                            _ => Err(Invalid),
                        })
                    },
                ),
                ParamType::Int => {
                    let (min, max, step) = param_range(param.kind, param.min, param.max);
                    number_row(Kind::Number { min, max, step })
                }
                ParamType::Float => {
                    let (min, max, step) = param_range(param.kind, param.min, param.max);
                    number_row(Kind::Slider { min, max, step })
                }
                ParamType::String => text_row(Kind::Text),
            }
        })
        .collect()
}

/// The recording, in the order of OBS: the folder, the picture, the encoder, the file, then
/// the encoder's own options when they are set by hand.
pub fn video(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let profile = config.cur();
    let hardware = profile.encoder_kind != "software";
    let list = encoders::choices(env, hardware);
    let current = encoders::resolved(env, &profile).map(|e| e.id.clone());

    let mut rows = vec![
        folder(
            "dir_videos",
            t(Key::SetFolder),
            |c| c.paths.videos.clone(),
            |c, v| c.paths.videos = v,
        ),
        toggle(
            "sub_videos",
            t(Key::SetSubfolder),
            |c| c.paths.per_app_subfolder.videos,
            |c, v| c.paths.per_app_subfolder.videos = v,
        ),
    ];

    // ---- the picture
    rows.push(header("h_video"));
    rows.push(choice(
        "resolution",
        t(Key::SetResolution),
        vec![
            opt("source", t(Key::SetResSource)),
            opt("2160p", "2160p"),
            opt("1440p", "1440p"),
            opt("1080p", "1080p"),
            opt("720p", "720p"),
            opt("480p", "480p"),
        ],
        |c| c.cur().resolution,
        |c, v| c.cur_mut().resolution = v,
    ));
    // Above 60 fps only what a monitor shows.
    let max = env.display.max_refresh.max(60);
    // (A 239.76 Hz monitor reads 239: one more lets 240 through.)
    let rates: Vec<u32> = FRAME_RATES
        .iter()
        .copied()
        .filter(|f| *f <= max + 1)
        .collect();
    let offered = rates.clone();
    rows.push(choice(
        "fps",
        t(Key::SetFps),
        rates
            .iter()
            .map(|f| opt(&f.to_string(), format!("{f} fps")))
            .collect(),
        move |c| fps_option(c.cur().fps, &offered).to_string(),
        |c, v| c.cur_mut().fps = v.parse().unwrap_or(60),
    ));
    rows.push(toggle(
        "video_cursor",
        t(Key::SetShowCursor),
        |c| c.cur().show_cursor,
        |c, v| c.cur_mut().show_cursor = v,
    ));

    // ---- the encoder
    rows.push(header("h_encoder"));
    rows.push(segmented(
        "encoder_kind",
        t(Key::SetEncoderKind),
        vec![
            opt("hardware", t(Key::SetKindHardware)),
            opt("software", t(Key::SetKindSoftware)),
        ],
        |c| c.cur().encoder_kind,
        |c, v| {
            let profile = c.cur_mut();
            profile.encoder_kind = v;
            // The best encoder of the new kind, resolved when it is read.
            profile.encoder = "auto".into();
        },
    ));
    let options: Vec<Opt> = list.iter().map(|e| opt(&e.id, e.name.as_str())).collect();
    let shown = current.clone();
    if list.is_empty() {
        // Nothing to choose from (yet): say why instead of an empty list.
        let why = if env.detecting && env.probe.is_none() {
            t(Key::SetDetecting)
        } else {
            t(Key::SetNoHardware)
        };
        rows.push(info("encoder", t(Key::SetEncoder), why));
    } else {
        rows.push(row(
            "encoder",
            t(Key::SetEncoder),
            Kind::Choice(options),
            Box::new(move |c| {
                // `auto` (older files) shows the encoder it resolves to.
                let own = c.cur().encoder;
                Value::Text(if own == "auto" {
                    shown.clone().unwrap_or_default()
                } else {
                    own
                })
            }),
            Box::new(|c, v| match v {
                Value::Text(id) => {
                    c.cur_mut().encoder = id;
                    Ok(())
                }
                _ => Err(Invalid),
            }),
        ));
    }
    rows.push(segmented(
        "preset",
        t(Key::SetPreset),
        vec![
            opt("quality", t(Key::SetPresetBest)),
            opt("small", t(Key::SetPresetLight)),
            opt("custom", t(Key::SetPresetCustom)),
        ],
        |c| match c.cur().preset.as_str() {
            "quality" | "small" => c.cur().preset,
            // The older presets were tuned by hand: they are the custom one now.
            _ => "custom".to_owned(),
        },
        |c, v| c.cur_mut().preset = v,
    ));
    let ten = current
        .as_deref()
        .is_some_and(|id| encoders::ten_bit(env, id));
    rows.push(when_boxed(
        toggle(
            "ten_bit",
            t(Key::SetTenBit),
            |c| c.cur().depth == 10,
            |c, v| c.cur_mut().depth = if v { 10 } else { 8 },
        ),
        // HDR needs 10 bits: the switch stays on, and cannot be turned off, while HDR is on.
        move |c| ten && !hdr_on(&c.cur().hdr),
    ));
    // Only when Windows shows HDR: there is nothing to keep otherwise.
    if env.display.hdr {
        rows.push(toggle(
            "hdr",
            t(Key::SetHdrEnable),
            |c| hdr_on(&c.cur().hdr),
            |c, v| {
                let profile = c.cur_mut();
                profile.hdr = if v { "keep_hdr" } else { "tonemap_sdr" }.into();
                if v {
                    profile.depth = 10;
                }
            },
        ));
    }

    // ---- the file
    rows.push(header("h_file"));
    rows.push(choice(
        "container",
        t(Key::SetContainer),
        // Fragmented MP4 only: a recording cut short (a crash, a full disk) still plays.
        vec![
            opt("mp4_fragmented", "MP4"),
            opt("mkv", "MKV"),
            opt("webm", "WebM"),
        ],
        |c| match c.cur().container.as_str() {
            "mp4" | "mp4_hybrid" => "mp4_fragmented".to_owned(),
            other => other.to_owned(),
        },
        |c, v| c.cur_mut().container = v,
    ));
    rows.push(choice(
        "split",
        t(Key::SetSplit),
        vec![
            opt("off", t(Key::SetSplitOff)),
            opt("size", t(Key::SetSplitSize)),
            opt("duration", t(Key::SetSplitDuration)),
        ],
        |c| split_parts(&c.cur().split.mode).0.to_owned(),
        |c, v| {
            let amount = split_parts(&c.cur().split.mode).1;
            let amount = if amount > 0 {
                amount
            } else if v == "size" {
                2_048
            } else {
                10
            };
            c.cur_mut().split.mode = split_join(&v, amount);
        },
    ));
    let mode = config.cur().split.mode;
    let split = split_parts(&mode).0;
    if split != "off" {
        rows.push(number(
            "split_amount",
            t(if split == "duration" {
                Key::SetSplitMinutes
            } else {
                Key::SetSplitSizeMb
            }),
            (1, 1_000_000, 1),
            |c| split_parts(&c.cur().split.mode).1,
            |c, v| {
                let kind = split_parts(&c.cur().split.mode).0.to_owned();
                c.cur_mut().split.mode = split_join(&kind, v);
            },
        ));
    }

    // ---- the encoder's options, set by hand
    if profile.preset == "custom" || !matches!(profile.preset.as_str(), "quality" | "small") {
        rows.push(header("h_custom"));
        if let Some(id) = &current {
            rows.extend(custom_rows(env, id));
        }
        rows.push(choice(
            "chroma",
            t(Key::SetChroma),
            vec![
                opt("420", "4:2:0"),
                opt("422", "4:2:2"),
                opt("444", "4:4:4"),
            ],
            |c| c.cur().chroma,
            |c, v| c.cur_mut().chroma = v,
        ));
    }
    rows
}

/// A switch that adds or removes `spec` from the sources of the profile.
fn source_row(spec: String, label: String) -> Row {
    let read = spec.clone();
    let write = spec.clone();
    row(
        format!("src:{spec}"),
        label,
        Kind::Toggle,
        Box::new(move |c| Value::Bool(c.cur().audio.sources.contains(&read))),
        Box::new(move |c, v| match v {
            Value::Bool(on) => {
                let sources = &mut c.cur_mut().audio.sources;
                sources.retain(|s| *s != write);
                if on {
                    sources.push(write.clone());
                }
                Ok(())
            }
            _ => Err(Invalid),
        }),
    )
}

/// A microphone source: `mic` (the Windows default) or `mic:<device id>`.
fn is_mic(spec: &str) -> bool {
    spec == "mic" || spec.starts_with("mic:")
}

/// The label of a source the machine does not offer right now.
fn absent_label(spec: &str) -> String {
    spec.split_once(':')
        .map_or(spec, |(_, name)| name)
        .to_owned()
}

/// A program the profile records, with a button to stop recording it.
fn program_row(spec: String, label: String, icon: Option<crate::Icon>) -> Row {
    let write = spec.clone();
    let mut r = row(
        format!("src:{spec}"),
        label,
        Kind::Removable,
        Box::new(|_| Value::Bool(true)),
        Box::new(move |c, v| match v {
            Value::Bool(false) => {
                c.cur_mut().audio.sources.retain(|s| *s != write);
                Ok(())
            }
            _ => Err(Invalid),
        }),
    );
    r.icon = icon;
    r
}

pub fn audio(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let chosen = config.cur().audio.sources;
    let mut rows = vec![source_row("system".into(), t(Key::SrcSystem))];
    // A particular output, from older settings: it stays visible so it can be turned off.
    for spec in chosen.iter().filter(|s| s.starts_with("out:")) {
        let label = env
            .audio
            .outputs
            .iter()
            .find(|d| format!("out:{}", d.id) == *spec)
            .map_or_else(|| absent_label(spec), |d| d.name.clone());
        rows.push(source_row(spec.clone(), label));
    }

    // ---- the microphone: a switch, then which one
    rows.push(header("h_mic"));
    rows.push(row(
        "mic_on",
        t(Key::SetMicOn),
        Kind::Toggle,
        Box::new(|c| Value::Bool(c.cur().audio.sources.iter().any(|s| is_mic(s)))),
        Box::new(|c, v| match v {
            Value::Bool(on) => {
                let sources = &mut c.cur_mut().audio.sources;
                if !on {
                    sources.retain(|s| !is_mic(s));
                } else if !sources.iter().any(|s| is_mic(s)) {
                    sources.push("mic".into());
                }
                Ok(())
            }
            _ => Err(Invalid),
        }),
    ));
    let mut mics = vec![opt("mic", t(Key::SrcMic))];
    mics.extend(
        env.audio
            .inputs
            .iter()
            .map(|d| opt(&format!("mic:{}", d.id), d.name.as_str())),
    );
    // An unplugged microphone that the profile records.
    for spec in chosen.iter().filter(|s| s.starts_with("mic:")) {
        if !mics.iter().any(|o| o.value == *spec) {
            mics.push(opt(
                spec,
                format!("{} ({})", absent_label(spec), t(Key::SrcAbsent)),
            ));
        }
    }
    rows.push(when(
        choice(
            "mic_device",
            t(Key::SetMicDevice),
            mics,
            |c| {
                c.cur()
                    .audio
                    .sources
                    .into_iter()
                    .find(|s| is_mic(s))
                    .unwrap_or_else(|| "mic".into())
            },
            // Greyed out while the microphone is off: nothing to change then.
            |c, v| {
                let sources = &mut c.cur_mut().audio.sources;
                if sources.iter().any(|s| is_mic(s)) {
                    sources.retain(|s| !is_mic(s));
                    sources.push(v);
                }
            },
        ),
        |c| c.cur().audio.sources.iter().any(|s| is_mic(s)),
    ));
    rows.push(when(
        toggle(
            "audio_denoise",
            t(Key::SetAudioDenoise),
            |c| c.cur().audio.mic_noise_reduction,
            |c, v| c.cur_mut().audio.mic_noise_reduction = v,
        ),
        |c| c.cur().audio.sources.iter().any(|s| is_mic(s)),
    ));

    // ---- the tracks and their codec
    rows.push(header("h_tracks"));
    rows.push(choice(
        "audio_routing",
        t(Key::SetAudioRouting),
        vec![
            opt("one_track_per_source", t(Key::SetRouteEach)),
            opt("mix_all", t(Key::SetRouteMix)),
            opt("advanced", t(Key::SetRouteAdvanced)),
        ],
        |c| c.cur().audio.routing,
        |c, v| c.cur_mut().audio.routing = v,
    ));
    rows.push(choice(
        "audio_codec",
        t(Key::SetAudioCodec),
        vec![
            opt("auto", t(Key::OptAuto)),
            opt("aac", "AAC"),
            opt("opus", "Opus"),
            opt("flac", t(Key::OptFlac)),
            opt("pcm16", t(Key::OptPcm16)),
            opt("pcm24", t(Key::OptPcm24)),
        ],
        |c| c.cur().audio.codec,
        |c, v| c.cur_mut().audio.codec = v,
    ));
    // A bitrate only means something for the compressed codecs (not FLAC or PCM).
    if matches!(config.cur().audio.codec.as_str(), "auto" | "aac" | "opus") {
        rows.push(slider(
            "audio_bitrate",
            t(Key::SetAudioBitrate),
            (32, 512, 16),
            |c| i64::from(c.cur().audio.bitrate_kbps),
            |c, v| c.cur_mut().audio.bitrate_kbps = v as u32,
        ));
        rows.push(toggle(
            "audio_vbr",
            t(Key::SetAudioVbr),
            |c| c.cur().audio.vbr,
            |c, v| c.cur_mut().audio.vbr = v,
        ));
    }
    rows.push(toggle(
        "audio_surround",
        t(Key::SetAudioSurround),
        |c| c.cur().audio.surround,
        |c, v| c.cur_mut().audio.surround = v,
    ));

    // ---- the programs recorded on their own, then a button to add one of those open now
    rows.push(header("h_programs"));
    let program_of = |spec: &str| {
        env.audio
            .programs
            .iter()
            .find(|p| format!("app:{}", p.id) == spec)
    };
    for spec in chosen.iter().filter(|s| s.starts_with("app:")) {
        let (label, icon) = program_of(spec).map_or_else(
            || (absent_label(spec), None),
            |p| (p.name.clone(), p.icon.clone()),
        );
        rows.push(program_row(spec.clone(), label, icon));
    }
    let offered: Vec<Opt> = env
        .audio
        .programs
        .iter()
        .map(|p| Opt {
            value: format!("app:{}", p.id),
            label: p.name.clone(),
            icon: p.icon.clone(),
        })
        .filter(|o| !chosen.contains(&o.value))
        .collect();
    rows.push(row(
        "add_program",
        t(Key::SetAddProgram),
        Kind::Add(offered),
        Box::new(|_| Value::Text(String::new())),
        Box::new(|c, v| match v {
            Value::Text(spec) => {
                let sources = &mut c.cur_mut().audio.sources;
                if !sources.contains(&spec) {
                    sources.push(spec);
                }
                Ok(())
            }
            _ => Err(Invalid),
        }),
    ));
    rows
}

/// The replay buffer: the last moments, kept to be saved on demand.
pub fn replay(env: &Env) -> Vec<Row> {
    let t = |k| env.t(k);
    vec![
        toggle(
            "replay_start",
            t(Key::SetReplayStart),
            |c| c.replay.enabled_on_start,
            |c, v| c.replay.enabled_on_start = v,
        ),
        slider(
            "replay_duration",
            t(Key::SetReplayDuration),
            (5, 1_200, 5),
            |c| i64::from(c.replay.duration_seconds),
            |c, v| c.replay.duration_seconds = v as u32,
        ),
        segmented(
            "replay_storage",
            t(Key::SetReplayStorage),
            vec![
                opt("ram", t(Key::SetStorageRam)),
                opt("disk", t(Key::SetStorageDisk)),
            ],
            |c| c.replay.storage.clone(),
            |c, v| c.replay.storage = v,
        ),
        header("h_folder"),
        folder(
            "dir_replays",
            t(Key::SetFolder),
            |c| c.paths.replays.clone(),
            |c, v| c.paths.replays = v,
        ),
        toggle(
            "sub_replays",
            t(Key::SetSubfolder),
            |c| c.paths.per_app_subfolder.replays,
            |c, v| c.paths.per_app_subfolder.replays = v,
        ),
    ]
}

/// The settings of the updates page (the state of the update is drawn by the page).
pub fn updates(env: &Env) -> Vec<Row> {
    vec![
        toggle(
            "check_updates",
            env.t(Key::SetCheckUpdates),
            |c| c.general.check_updates,
            |c, v| c.general.check_updates = v,
        ),
        when(
            toggle(
                "auto_update",
                env.t(Key::SetAutoUpdate),
                |c| c.general.auto_update,
                |c, v| c.general.auto_update = v,
            ),
            |c| c.general.check_updates,
        ),
    ]
}
