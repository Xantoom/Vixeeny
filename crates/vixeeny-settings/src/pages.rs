// SPDX-License-Identifier: GPL-3.0-or-later
//! The rows of each page of the settings window.

use vixeeny_common::config::Config;
use vixeeny_common::i18n::Key;
use vixeeny_encode::registry::ParamType;

use crate::encoders::{self, param_label, param_range};
use crate::{
    Current, Env, Invalid, Kind, Opt, Row, Value, choice, folder, header, hinted, info, list,
    number, opt, row, segmented, slider, text, toggle, unlist, when, when_boxed,
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

fn app_names_text(c: &Config) -> String {
    c.paths
        .app_names
        .iter()
        .map(|(exe, name)| format!("{exe}={name}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_app_names(text: &str) -> std::collections::BTreeMap<String, String> {
    text.split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(exe, name)| (exe.trim().to_owned(), name.trim().to_owned()))
        .filter(|(exe, name)| !exe.is_empty() && !name.is_empty())
        .collect()
}

/// The frame rate offered closest to `fps` (a setting from before the shorter list).
const fn fps_option(fps: u32, high: bool) -> u32 {
    if fps >= 90 && high {
        120
    } else if fps >= 45 {
        60
    } else {
        30
    }
}

fn hdr_on(setting: &str) -> bool {
    matches!(setting, "keep_hdr" | "hdr")
}

pub fn general(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let mut rows = vec![
        header("h_app", t(Key::GrpApplication)),
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
        header("h_overlay", t(Key::GrpOverlay)),
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
    rows.extend([
        header("h_naming", t(Key::GrpNaming)),
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
        hinted(
            text("app_names", t(Key::SetAppNames), app_names_text, |c, v| {
                c.paths.app_names = parse_app_names(&v);
            }),
            t(Key::SetAppNamesHint),
        ),
        header("h_subfolders", t(Key::GrpSubfolders)),
        toggle(
            "sub_images",
            t(Key::SetSubImages),
            |c| c.paths.per_app_subfolder.images,
            |c, v| c.paths.per_app_subfolder.images = v,
        ),
        toggle(
            "sub_videos",
            t(Key::SetSubVideos),
            |c| c.paths.per_app_subfolder.videos,
            |c, v| c.paths.per_app_subfolder.videos = v,
        ),
        toggle(
            "sub_replays",
            t(Key::SetSubReplays),
            |c| c.paths.per_app_subfolder.replays,
            |c, v| c.paths.per_app_subfolder.replays = v,
        ),
        header("h_advanced", t(Key::GrpAdvanced)),
        hinted(
            number(
                "idle_exit",
                t(Key::SetIdleExit),
                (0, 600, 5),
                |c| i64::from(c.general.app_idle_exit_seconds),
                |c, v| c.general.app_idle_exit_seconds = v as u32,
            ),
            t(Key::SetIdleExitHint),
        ),
    ]);
    rows
}

/// Screenshots: format and its options, capture, folder, text recognition.
pub fn capture(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let mut rows = vec![
        header("h_format", t(Key::GrpFormat)),
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
        header("h_capture", t(Key::GrpCapture)),
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
        hinted(
            toggle(
                "image_hdr",
                t(Key::SetHdrEnable),
                |c| matches!(c.image.hdr.as_str(), "keep_hdr" | "hdr"),
                |c, v| c.image.hdr = if v { "keep_hdr" } else { "tonemap_sdr" }.into(),
            ),
            t(Key::SetHdrEnableHint),
        ),
        slider(
            "dim",
            t(Key::SetDim),
            (0, 90, 5),
            |c| i64::from(c.editor.dim_percent),
            |c, v| c.editor.dim_percent = v as u8,
        ),
        header("h_location", t(Key::GrpLocation)),
        folder(
            "dir_images",
            t(Key::SetDirImages),
            |c| c.paths.images.clone(),
            |c, v| c.paths.images = v,
        ),
        header("h_ocr", t(Key::GrpOcr)),
        hinted(
            text(
                "ocr_languages",
                t(Key::SetOcrLanguages),
                |c| list(&c.ocr.languages),
                |c, v| {
                    let languages = unlist(&v);
                    c.ocr.languages = if languages.is_empty() {
                        vec!["auto".into()]
                    } else {
                        languages
                    };
                },
            ),
            t(Key::SetOcrLanguagesHint),
        ),
    ]);
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

pub fn video(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let profile = config.cur();
    let hardware = profile.encoder_kind != "software";
    let list = encoders::choices(env, hardware);
    let current = encoders::resolved(env, &profile).map(|e| e.id.clone());

    let names = crate::profiles::names(config);
    let mut rows = vec![
        header("h_profile", t(Key::GrpProfile)),
        hinted(
            row(
                "profile",
                t(Key::SetProfile),
                Kind::Profile(names),
                Box::new(|c| Value::Text(c.video.profile.clone())),
                Box::new(|c, v| match v {
                    Value::Text(name) => {
                        c.video.profile = name;
                        Ok(())
                    }
                    _ => Err(Invalid),
                }),
            ),
            t(Key::SetProfileHint),
        ),
    ];

    rows.push(header("h_encoder", t(Key::GrpEncoder)));
    rows.push(hinted(
        segmented(
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
        ),
        t(Key::SetEncoderKindHint),
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
        rows.push(hinted(
            row(
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
            ),
            t(Key::SetEncoderHint),
        ));
    }
    if hardware && let Some(probe) = &env.probe {
        let cards: Vec<&str> = probe
            .adapters
            .iter()
            .filter(|a| !a.software)
            .map(|a| a.name.as_str())
            .collect();
        if !cards.is_empty() {
            rows.push(info("gpu", t(Key::SetGpu), cards.join("\n")));
        }
    }
    rows.push(hinted(
        segmented(
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
        ),
        t(Key::SetPresetHint),
    ));

    rows.push(header("h_video", t(Key::GrpVideo)));
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
    // 120 fps only where a monitor shows that many.
    let high = env.display.max_refresh >= 120;
    let rates: &[u32] = if high { &[30, 60, 120] } else { &[30, 60] };
    rows.push(segmented(
        "fps",
        t(Key::SetFps),
        rates
            .iter()
            .map(|f| opt(&f.to_string(), format!("{f} fps")))
            .collect(),
        move |c| fps_option(c.cur().fps, high).to_string(),
        |c, v| c.cur_mut().fps = v.parse().unwrap_or(60),
    ));
    let ten = current
        .as_deref()
        .is_some_and(|id| encoders::ten_bit(env, id));
    rows.push(when_boxed(
        hinted(
            toggle(
                "ten_bit",
                t(Key::SetTenBit),
                |c| c.cur().depth == 10,
                |c, v| c.cur_mut().depth = if v { 10 } else { 8 },
            ),
            t(Key::SetTenBitHint),
        ),
        // HDR needs 10 bits: the switch stays on, and cannot be turned off, while HDR is on.
        move |c| ten && !hdr_on(&c.cur().hdr),
    ));
    // HDR needs Windows to show it: without, the switch is off and greyed out.
    let hdr_display = env.display.hdr;
    rows.push(when_boxed(
        hinted(
            toggle(
                "hdr",
                t(Key::SetHdrEnable),
                move |c| hdr_display && hdr_on(&c.cur().hdr),
                |c, v| {
                    let profile = c.cur_mut();
                    profile.hdr = if v { "keep_hdr" } else { "tonemap_sdr" }.into();
                    if v {
                        profile.depth = 10;
                    }
                },
            ),
            if hdr_display {
                t(Key::SetHdrEnableHint)
            } else {
                t(Key::SetHdrOffInWindows)
            },
        ),
        move |_| hdr_display,
    ));
    rows.push(toggle(
        "video_cursor",
        t(Key::SetShowCursor),
        |c| c.cur().show_cursor,
        |c, v| c.cur_mut().show_cursor = v,
    ));
    rows.push(toggle(
        "vfr",
        t(Key::SetVfr),
        |c| c.cur().vfr,
        |c, v| c.cur_mut().vfr = v,
    ));

    if profile.preset == "custom" || !matches!(profile.preset.as_str(), "quality" | "small") {
        if let Some(id) = &current {
            let custom = custom_rows(env, id);
            if !custom.is_empty() {
                rows.push(header("h_custom", t(Key::GrpCustom)));
                rows.extend(custom);
            }
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

    rows.push(header("h_split", t(Key::GrpSplit)));
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

    rows.push(header("h_location", t(Key::GrpLocation)));
    rows.push(folder(
        "dir_videos",
        t(Key::SetDirVideos),
        |c| c.paths.videos.clone(),
        |c, v| c.paths.videos = v,
    ));
    rows.extend(replay(env, config));
    rows
}

/// A switch that adds or removes `spec` from the sources of the profile.
fn source_row(spec: String, label: String, hint: String) -> Row {
    let read = spec.clone();
    let write = spec.clone();
    hinted(
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
        ),
        hint,
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

pub fn audio(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let chosen = config.cur().audio.sources;
    let mut rows = Vec::new();

    // ---- what the PC plays
    rows.push(header("h_pc", t(Key::GrpPcSound)));
    rows.push(source_row(
        "system".into(),
        t(Key::SrcSystem),
        t(Key::SrcSystemHint),
    ));
    // A particular output, from older settings: it stays visible so it can be turned off.
    for spec in chosen.iter().filter(|s| s.starts_with("out:")) {
        let label = env
            .audio
            .outputs
            .iter()
            .find(|d| format!("out:{}", d.id) == *spec)
            .map_or_else(|| absent_label(spec), |d| d.name.clone());
        rows.push(source_row(spec.clone(), label, String::new()));
    }

    // ---- the microphone: a switch, then which one
    rows.push(header("h_mic", t(Key::GrpMic)));
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

    // ---- the programs open now that have sound, and those the profile records
    let programs: Vec<(String, String, String)> = env
        .audio
        .programs
        .iter()
        .map(|p| (format!("app:{}", p.id), p.name.clone(), String::new()))
        .chain(
            chosen
                .iter()
                .filter(|s| s.starts_with("app:"))
                .filter(|s| {
                    !env.audio
                        .programs
                        .iter()
                        .any(|p| format!("app:{}", p.id) == **s)
                })
                .map(|s| (s.clone(), absent_label(s), t(Key::SrcAbsent))),
        )
        .collect();
    rows.push(header("h_programs", t(Key::GrpPrograms)));
    rows.push(info(
        "programs_hint",
        if programs.is_empty() {
            t(Key::SrcNoPrograms)
        } else {
            t(Key::SrcProgramsHint)
        },
        String::new(),
    ));
    for (spec, label, hint) in programs {
        rows.push(source_row(spec, label, hint));
    }

    rows.push(header("h_tracks", t(Key::GrpTracks)));
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
    rows.push(toggle(
        "audio_surround",
        t(Key::SetAudioSurround),
        |c| c.cur().audio.surround,
        |c, v| c.cur_mut().audio.surround = v,
    ));
    rows
}

/// The replay buffer, at the end of the video page.
fn replay(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let mut profiles = vec![opt("", t(Key::SetSameAsRecording))];
    profiles.extend(config.profiles.keys().map(|name| opt(name, name.as_str())));
    vec![
        header("h_replay", t(Key::GrpReplay)),
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
        choice(
            "replay_profile",
            t(Key::SetReplayProfile),
            profiles,
            |c| c.replay.profile.clone(),
            |c, v| c.replay.profile = v,
        ),
        folder(
            "dir_replays",
            t(Key::SetDirReplays),
            |c| c.paths.replays.clone(),
            |c, v| c.paths.replays = v,
        ),
    ]
}

/// The settings of the about page (the version and the update card are drawn by the page).
pub fn about(env: &Env) -> Vec<Row> {
    vec![
        header("h_updates", env.t(Key::GrpUpdates)),
        toggle(
            "check_updates",
            env.t(Key::SetCheckUpdates),
            |c| c.general.check_updates,
            |c, v| c.general.check_updates = v,
        ),
        when(
            hinted(
                toggle(
                    "auto_update",
                    env.t(Key::SetAutoUpdate),
                    |c| c.general.auto_update,
                    |c, v| c.general.auto_update = v,
                ),
                env.t(Key::SetAutoUpdateHint),
            ),
            |c| c.general.check_updates,
        ),
    ]
}
