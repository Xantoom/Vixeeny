// SPDX-License-Identifier: GPL-3.0-or-later
//! The rows of each page of the settings window.

use vixeeny_common::config::{Config, Naming, TARGET_SOURCE};
use vixeeny_common::i18n::Key;
use vixeeny_encode::registry::{
    BITRATE_RANGE, DEFAULT_BITRATE, DEFAULT_MAXRATE, Family, ParamType, RateMode, rate_keys,
};
use vixeeny_encode::{replay, validate};

use crate::encoders::{self, param_label, param_range};
use crate::{
    Env, Invalid, Kind, Opt, Row, Value, choice, folder, header, hinted, info, number, opt, row,
    slider, toggle,
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
        choice(
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
    ]
}

/// How the files of one kind are named. `get` and `naming` reach the settings of that kind.
fn naming_rows(
    env: &Env,
    kind: &str,
    get: fn(&Config) -> &Naming,
    naming: fn(&mut Config) -> &mut Naming,
) -> [Row; 1] {
    let template = row(
        format!("template:{kind}"),
        env.t(Key::SetTemplate),
        Kind::Text,
        Box::new(move |c| Value::Text(get(c).template.clone())),
        Box::new(move |c, v| match v {
            Value::Text(v) => {
                // The template cannot be emptied.
                if !v.trim().is_empty() {
                    naming(c).template = v;
                }
                Ok(())
            }
            _ => Err(Invalid),
        }),
    );
    [hinted(template, env.t(Key::SetTemplateHint))]
}

/// The side strip and the recording widget.
pub fn overlay(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let mut rows = vec![
        crate::picked(
            crate::Picker::Edge,
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
        rows.push(crate::picked(
            crate::Picker::Corner,
            choice(
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
            ),
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
    ];
    rows.extend(naming_rows(
        env,
        "images",
        |c| &c.paths.naming.images,
        |c| &mut c.paths.naming.images,
    ));
    rows.extend([
        header("h_format"),
        choice(
            "image_format",
            t(Key::SetImageFormat),
            vec![
                crate::heading(t(Key::OptGroupCompatible)),
                opt("png", "PNG"),
                opt("jpeg", "JPEG"),
                crate::heading(t(Key::OptGroupModern)),
                opt("webp", "WebP"),
                opt("avif", "AVIF"),
                opt("jxl", "JPEG XL"),
            ],
            |c| c.image.format.clone(),
            |c, v| c.image.format = v,
        ),
    ]);
    // The options of the chosen format's encoder.
    match config.image.format.as_str() {
        "png" => {
            rows.push(choice(
                "png_compression",
                t(Key::SetPngCompression),
                vec![
                    opt("fast", t(Key::OptPngFast)),
                    opt("default", t(Key::OptPngDefault)),
                    opt("high", t(Key::OptPngHigh)),
                ],
                |c| c.image.png.compression.clone(),
                |c, v| c.image.png.compression = v,
            ));
            rows.push(choice(
                "png_optimize",
                t(Key::SetPngOptimize),
                vec![
                    opt("0", t(Key::OptOff)),
                    opt("2", t(Key::OptPngFast)),
                    opt("4", t(Key::OptPngDefault)),
                    opt("6", t(Key::OptPngHigh)),
                ],
                |c| {
                    match c.image.png.optimize {
                        0 => "0",
                        1..=2 => "2",
                        3..=4 => "4",
                        _ => "6",
                    }
                    .to_owned()
                },
                |c, v| c.image.png.optimize = v.parse().unwrap_or(0),
            ));
        }
        "jpeg" => {
            rows.push(slider(
                "jpeg_quality",
                t(Key::SetQuality),
                (1, 100, 1),
                |c| i64::from(c.image.jpeg.quality),
                |c, v| c.image.jpeg.quality = v as u8,
            ));
        }
        "webp" => {
            rows.push(toggle(
                "webp_lossless",
                t(Key::SetLossless),
                |c| c.image.webp.lossless,
                |c, v| c.image.webp.lossless = v,
            ));
            if !config.image.webp.lossless {
                rows.push(slider(
                    "webp_quality",
                    t(Key::SetQuality),
                    (0, 100, 1),
                    |c| i64::from(c.image.webp.quality),
                    |c, v| c.image.webp.quality = v as u8,
                ));
            }
            rows.push(hinted(
                slider(
                    "webp_effort",
                    t(Key::SetEffort),
                    (0, 6, 1),
                    |c| i64::from(c.image.webp.effort),
                    |c, v| c.image.webp.effort = v as u8,
                ),
                t(Key::SetEffortHint),
            ));
        }
        "avif" => {
            rows.push(slider(
                "avif_quality",
                t(Key::SetQuality),
                (0, 100, 1),
                |c| i64::from(c.image.avif.quality),
                |c, v| c.image.avif.quality = v as u8,
            ));
            rows.push(choice(
                "avif_depth",
                t(Key::SetAvifDepth),
                vec![opt("8", "8 bits"), opt("10", "10 bits")],
                |c| c.image.avif.depth.to_string(),
                |c, v| c.image.avif.depth = v.parse().unwrap_or(10),
            ));
            rows.push(hinted(
                slider(
                    "avif_speed",
                    t(Key::SetAvifSpeed),
                    (0, 10, 1),
                    |c| i64::from(c.image.avif.speed),
                    |c, v| c.image.avif.speed = v as u8,
                ),
                t(Key::SetAvifSpeedHint),
            ));
        }
        "jxl" => {
            rows.push(toggle(
                "jxl_lossless",
                t(Key::SetLossless),
                |c| c.image.jxl.lossless,
                |c, v| c.image.jxl.lossless = v,
            ));
            if !config.image.jxl.lossless {
                rows.push(slider(
                    "jxl_quality",
                    t(Key::SetQuality),
                    (1, 100, 1),
                    |c| i64::from(c.image.jxl.quality),
                    |c, v| c.image.jxl.quality = v as u8,
                ));
            }
            rows.push(hinted(
                slider(
                    "jxl_effort",
                    t(Key::SetEffort),
                    (1, 9, 1),
                    |c| i64::from(c.image.jxl.effort),
                    |c, v| c.image.jxl.effort = v as u8,
                ),
                t(Key::SetEffortHint),
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
/// A custom option, stored as text under `key` in the profile's `params`.
fn param_row(id: String, label: String, kind: Kind, key: String, default: String) -> Row {
    let read = key.clone();
    let numeric = matches!(kind, Kind::Number { .. } | Kind::Slider { .. });
    let toggle = matches!(kind, Kind::Toggle);
    row(
        id,
        label,
        kind,
        Box::new(move |c| {
            let v = c.video.params.get(&read).unwrap_or(&default);
            if numeric {
                Value::Int(v.parse::<f64>().map_or(0, |n| n as i64))
            } else if toggle {
                Value::Bool(matches!(v.as_str(), "1" | "true"))
            } else {
                Value::Text(v.clone())
            }
        }),
        Box::new(move |c, v| {
            let v = match v {
                Value::Int(n) => n.to_string(),
                Value::Bool(b) => u8::from(b).to_string(),
                Value::Text(t) => t,
            };
            c.video.params.insert(key.clone(), v);
            Ok(())
        }),
    )
}

/// The options of the encoder set by hand: the rate control (only the fields of the chosen
/// mode) and the speed / quality preset, then the encoder's other parameters (the expert
/// ones).
fn custom_rows(env: &Env, config: &Config, encoder: &str) -> (Vec<Row>, Vec<Row>) {
    let Some(spec) = encoders::spec(encoder) else {
        return (Vec::new(), Vec::new());
    };
    let lang = env.lang;
    let rc = &spec.rate_control;
    let mode = spec.rate_mode(&config.video.params);
    let mut rows = Vec::new();
    // Grouped: aiming at a quality, aiming at a bitrate.
    let mut modes: Vec<Opt> = Vec::new();
    for (bitrate, title) in [(false, Key::OptGroupQuality), (true, Key::OptGroupBitrate)] {
        let group: Vec<Opt> = spec
            .rate_modes()
            .filter(|m| m.has_bitrate() == bitrate)
            .map(|m| opt(m.name(), encoders::rate_mode_label(m, spec, lang)))
            .collect();
        if !group.is_empty() {
            modes.push(crate::heading(env.t(title)));
            modes.extend(group);
        }
    }
    if modes.len() > 2 {
        rows.push(param_row(
            format!("p:{}", rate_keys::MODE),
            param_label(rate_keys::MODE, lang),
            Kind::Choice(modes),
            rate_keys::MODE.into(),
            RateMode::Quality.name().into(),
        ));
    }
    let scale = match mode {
        RateMode::Quality => Some((rate_keys::QUALITY, &rc.quality)),
        RateMode::Cqp => rc.qp.as_ref().map(|q| (rate_keys::QP, q)),
        RateMode::Vbr | RateMode::Cbr => None,
    };
    if let Some((key, q)) = scale {
        let label = if lang == vixeeny_common::i18n::Lang::Fr {
            "Qualité"
        } else {
            "Quality"
        };
        rows.push(hinted(
            param_row(
                format!("p:{key}"),
                label.into(),
                Kind::Slider {
                    min: q.min,
                    max: q.max,
                    step: 1,
                },
                key.into(),
                q.default.to_string(),
            ),
            encoders::lower_is_better(lang).into(),
        ));
    } else {
        let (min, max) = BITRATE_RANGE;
        let mut rate = |key: &str, default: i64| {
            rows.push(param_row(
                format!("p:{key}"),
                param_label(key, lang),
                Kind::Number {
                    min,
                    max,
                    step: 500,
                },
                key.into(),
                default.to_string(),
            ));
        };
        rate(rate_keys::BITRATE, DEFAULT_BITRATE);
        if rc.uses(mode, "{maxrate}") {
            rate(rate_keys::MAXRATE, DEFAULT_MAXRATE);
        }
    }
    let mut expert = Vec::new();
    for param in &spec.params {
        let (min, max, step) = param_range(param.min, param.max);
        let (label, kind) = match param.kind {
            ParamType::Enum => (
                param_label(&param.key, lang),
                Kind::Choice(
                    param
                        .values
                        .iter()
                        .map(|v| opt(v, encoders::value_label(&param.key, v, lang)))
                        .collect(),
                ),
            ),
            ParamType::Bool => (param_label(&param.key, lang), Kind::Toggle),
            // A numbered preset reads better on a slider, with what its ends mean.
            ParamType::Int if param.key == "preset" => (
                encoders::numbered_preset_label(min, max, lang),
                Kind::Slider { min, max, step },
            ),
            ParamType::Int => (
                param_label(&param.key, lang),
                Kind::Number { min, max, step },
            ),
            ParamType::Float => (
                param_label(&param.key, lang),
                Kind::Slider { min, max, step },
            ),
            ParamType::String => (param_label(&param.key, lang), Kind::Text),
        };
        let row = param_row(
            format!("p:{}", param.key),
            label,
            kind,
            param.key.clone(),
            param.default.to_ffmpeg(),
        );
        // The speed / quality trade-off is the one everybody may want to change.
        if matches!(param.key.as_str(), "preset" | "quality" | "deadline") {
            rows.push(row);
        } else {
            expert.push(row);
        }
    }
    (rows, expert)
}

/// The recording, in the order of OBS: the folder, the picture, the encoder, the file, then
/// the encoder's own options when they are set by hand.
pub fn video(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let profile = &config.video;
    let hardware = profile.encoder_kind != "software";
    let list = encoders::choices(env, hardware);
    let current = encoders::resolved(env, profile).map(|e| e.id.clone());

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
    rows.extend(naming_rows(
        env,
        "videos",
        |c| &c.paths.naming.videos,
        |c| &mut c.paths.naming.videos,
    ));

    // ---- the picture
    rows.push(header("h_video"));
    let screen = main_screen(env);
    let ratio = ratio_name(screen);
    // A screen that is not 16:9 (ultrawide, 16:10...) can be recorded as 16:9.
    if ratio != "16:9" || profile.aspect != "source" {
        let aspect = choice(
            "aspect",
            t(Key::SetAspect),
            vec![
                opt("source", t(Key::OptAspectScreen).replace("{ratio}", &ratio)),
                opt("16:9", t(Key::OptAspectCrop).replace("{ratio}", "16:9")),
            ],
            |c| match c.video.aspect.as_str() {
                "16:9" => "16:9".to_owned(),
                _ => "source".to_owned(),
            },
            |c, v| c.video.aspect = v,
        );
        rows.push(if profile.aspect == "source" {
            aspect
        } else {
            hinted(aspect, t(Key::SetAspectHint))
        });
    }
    let (_, _, cw, ch) = validate::center_crop(screen, validate::aspect(&profile.aspect));
    let mut sizes = vec![opt("source", size_name((cw, ch)))];
    for height in [2160u32, 1440, 1080, 720, 480] {
        let value = format!("{height}p");
        // Only smaller than the screen (the setting of another machine stays listed).
        if height < ch || profile.resolution == value {
            let size = validate::output_size(&value, (cw, ch)).unwrap_or((cw, ch));
            sizes.push(opt(&value, size_name(size)));
        }
    }
    rows.push(choice(
        "resolution",
        t(Key::SetResolution),
        sizes,
        |c| c.video.resolution.clone(),
        |c, v| c.video.resolution = v,
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
        move |c| fps_option(c.video.fps, &offered).to_string(),
        |c, v| c.video.fps = v.parse().unwrap_or(60),
    ));
    rows.push(toggle(
        "video_cursor",
        t(Key::SetShowCursor),
        |c| c.video.show_cursor,
        |c, v| c.video.show_cursor = v,
    ));

    // ---- the encoder
    rows.push(header("h_encoder"));
    rows.push(choice(
        "encoder_kind",
        t(Key::SetEncoderKind),
        vec![
            opt("hardware", t(Key::SetKindHardware)),
            opt("software", t(Key::SetKindSoftware)),
        ],
        |c| c.video.encoder_kind.clone(),
        |c, v| {
            let profile = &mut c.video;
            profile.encoder_kind = v;
            // The best encoder of the new kind, resolved when it is read.
            profile.encoder = "auto".into();
        },
    ));
    let recommended = encoders::best(env, hardware).map(|e| e.id.clone());
    // One section per maker when there are several chips (a graphics card and the processor's).
    let makers = list
        .iter()
        .map(|e| e.vendor)
        .collect::<std::collections::BTreeSet<_>>();
    let mut options: Vec<Opt> = Vec::new();
    for e in &list {
        if makers.len() > 1 && !options.iter().any(|o| o.heading && o.label == e.vendor) {
            options.push(crate::heading(e.vendor));
        }
        options.push(if recommended.as_deref() == Some(e.id.as_str()) {
            opt(&e.id, format!("{} ({})", e.name, t(Key::OptRecommended)))
        } else {
            opt(&e.id, e.name.as_str())
        });
    }
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
                let own = c.video.encoder.clone();
                Value::Text(if own == "auto" {
                    shown.clone().unwrap_or_default()
                } else {
                    own
                })
            }),
            Box::new(|c, v| match v {
                Value::Text(id) => {
                    c.video.encoder = id;
                    Ok(())
                }
                _ => Err(Invalid),
            }),
        ));
    }
    rows.push(choice(
        "preset",
        t(Key::SetPreset),
        vec![
            opt("quality", t(Key::SetPresetBest)),
            opt("small", t(Key::SetPresetLight)),
            opt("custom", t(Key::SetPresetCustom)),
        ],
        |c| match c.video.preset.as_str() {
            "quality" | "small" => c.video.preset.clone(),
            // The older presets were tuned by hand: they are the custom one now.
            _ => "custom".to_owned(),
        },
        |c, v| c.video.preset = v,
    ));
    // Only when Windows shows HDR: there is nothing to keep otherwise.
    if env.display.hdr {
        rows.push(toggle(
            "hdr",
            t(Key::SetHdrEnable),
            |c| hdr_on(&c.video.hdr),
            |c, v| c.video.hdr = if v { "keep_hdr" } else { "tonemap_sdr" }.into(),
        ));
    }

    // ---- the file
    rows.push(header("h_file"));
    rows.push(choice(
        "container",
        t(Key::SetContainer),
        // Fragmented MP4 only: a recording cut short (a crash, a full disk) still plays.
        vec![
            opt("mp4_fragmented", t(Key::OptMp4Fragmented)),
            opt("mkv", "MKV"),
            opt("webm", "WebM"),
        ],
        |c| match c.video.container.as_str() {
            "mp4" | "mp4_hybrid" => "mp4_fragmented".to_owned(),
            other => other.to_owned(),
        },
        |c, v| c.video.container = v,
    ));
    rows.push(choice(
        "split",
        t(Key::SetSplit),
        vec![
            opt("off", t(Key::SetSplitOff)),
            opt("size", t(Key::SetSplitSize)),
            opt("duration", t(Key::SetSplitDuration)),
        ],
        |c| split_parts(&c.video.split.mode).0.to_owned(),
        |c, v| {
            let amount = split_parts(&c.video.split.mode).1;
            let amount = if amount > 0 {
                amount
            } else if v == "size" {
                2_048
            } else {
                10
            };
            c.video.split.mode = split_join(&v, amount);
        },
    ));
    let mode = &config.video.split.mode;
    let split = split_parts(mode).0;
    if split != "off" {
        rows.push(number(
            "split_amount",
            t(if split == "duration" {
                Key::SetSplitMinutes
            } else {
                Key::SetSplitSizeMb
            }),
            (1, 1_000_000, 1),
            |c| split_parts(&c.video.split.mode).1,
            |c, v| {
                let kind = split_parts(&c.video.split.mode).0.to_owned();
                c.video.split.mode = split_join(&kind, v);
            },
        ));
    }

    // ---- the encoder's options, set by hand
    if profile.preset == "custom" || !matches!(profile.preset.as_str(), "quality" | "small") {
        rows.push(header("h_custom"));
        let (basic, expert) = current
            .as_deref()
            .map(|id| custom_rows(env, config, id))
            .unwrap_or_default();
        rows.extend(basic);
        let (row, open) = crate::expander(env, "expert_video", t(Key::SetExpert));
        rows.push(hinted(row, t(Key::SetExpertHint)));
        if open {
            rows.extend(expert);
            rows.push(choice(
                "chroma",
                t(Key::SetChroma),
                vec![
                    opt("420", "4:2:0"),
                    opt("422", "4:2:2"),
                    opt("444", "4:4:4"),
                ],
                |c| c.video.chroma.clone(),
                |c, v| c.video.chroma = v,
            ));
        }
    }
    rows
}

/// The screen recordings are measured on: the main one (1920 × 1080 until it is known).
pub(crate) fn main_screen(env: &Env) -> (u32, u32) {
    let screens = &env.machine.screens;
    screens
        .iter()
        .find(|s| s.primary)
        .or_else(|| screens.first())
        .map_or((1920, 1080), |s| (s.width, s.height))
}

/// The usual name of a picture ratio (`21:9` for 3440 × 1440), else the reduced fraction.
fn ratio_name((w, h): (u32, u32)) -> String {
    let r = f64::from(w) / f64::from(h.max(1));
    let named = [(16, 9), (16, 10), (21, 9), (32, 9), (4, 3), (5, 4), (3, 2)];
    // Ultrawide screens are 43:18 or 64:27, both sold as 21:9.
    if let Some((a, b)) = named.into_iter().find(|(a, b)| {
        let n = f64::from(*a) / f64::from(*b);
        let tolerance = if *a == 21 { 0.05 } else { 0.01 };
        (r - n).abs() / n < tolerance
    }) {
        return format!("{a}:{b}");
    }
    let gcd = |mut a: u32, mut b: u32| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a.max(1)
    };
    let g = gcd(w, h);
    format!("{}:{}", w / g, h / g)
}

/// `3840 × 2160`, with its usual name for the 16:9 sizes.
fn size_name((w, h): (u32, u32)) -> String {
    let name = match (w, h) {
        (3840, 2160) => "4K UHD",
        (2560, 1440) => "QHD",
        (1920, 1080) => "Full HD",
        (1280, 720) => "HD",
        _ => "",
    };
    if !name.is_empty() {
        format!("{w} × {h} ({name})")
    } else {
        format!("{w} × {h}")
    }
}

/// The audio bitrates offered, in kbit/s (160 is recommended).
const AUDIO_BITRATES: [u32; 6] = [96, 128, 160, 192, 256, 320];

/// The offered audio bitrate closest to `kbps` (a setting from an older version).
fn audio_bitrate_option(kbps: u32) -> u32 {
    AUDIO_BITRATES
        .iter()
        .copied()
        .min_by_key(|k| k.abs_diff(kbps))
        .unwrap_or(160)
}

/// The audio codec `auto` stands for: Opus beside AV1 (and VP9, and in WebM), else AAC.
fn auto_audio_codec(env: &Env, config: &Config) -> &'static str {
    let family = encoders::resolved(env, &config.video)
        .and_then(|e| encoders::spec(&e.id))
        .map(|spec| spec.family);
    if matches!(family, Some(Family::Av1 | Family::Vp9)) || config.video.container == "webm" {
        "opus"
    } else {
        "aac"
    }
}

/// One audio device recorded on its own: an output (`out:<id>`) or an input (`in:<id>`).
fn is_device(spec: &str) -> bool {
    spec.starts_with("out:") || spec.starts_with("in:")
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

/// What the profile records besides the microphone: `system` (all the PC sound), `target` (the
/// recorded program), `output` (one output device), `programs` (the ones ticked) or `none`.
/// Settings from before 0.9.15 have no such field: it is read from their sources.
fn capture_mode(config: &Config) -> &str {
    let audio = &config.video.audio;
    if matches!(audio.capture.as_str(), "programs" | "none") {
        return &audio.capture;
    }
    let sources = &audio.sources;
    if sources.iter().any(|s| s == TARGET_SOURCE) {
        "target"
    } else if sources.iter().any(|s| is_device(s)) {
        "output"
    } else if sources.iter().any(|s| s.starts_with("app:")) {
        "programs"
    } else if sources.iter().any(|s| s == "system") {
        "system"
    } else {
        "none"
    }
}

/// Switches what is recorded besides the microphone (the microphone is kept as it is).
fn set_capture(config: &mut Config, mode: &str, first_output: Option<&str>) {
    let audio = &mut config.video.audio;
    let kept_output = audio.sources.iter().find(|s| is_device(s)).cloned();
    audio.sources.retain(|s| is_mic(s));
    match mode {
        "system" => audio.sources.push("system".into()),
        "target" => audio.sources.push(TARGET_SOURCE.into()),
        "output" => {
            if let Some(out) = kept_output.or_else(|| first_output.map(str::to_owned)) {
                audio.sources.push(out);
            }
        }
        _ => {}
    }
    audio.capture = mode.to_owned();
}

/// A program of the list, ticked when the profile records it.
fn program_row(spec: String, label: String, icon: Option<crate::Icon>) -> Row {
    let read = spec.clone();
    let mut r = row(
        format!("src:{spec}"),
        label,
        Kind::Toggle,
        Box::new(move |c| Value::Bool(c.video.audio.sources.contains(&read))),
        Box::new(move |c, v| match v {
            Value::Bool(on) => {
                let sources = &mut c.video.audio.sources;
                sources.retain(|s| *s != spec);
                if on {
                    sources.push(spec.clone());
                }
                Ok(())
            }
            _ => Err(Invalid),
        }),
    );
    r.icon = icon;
    r
}

/// The volume keys of the microphone and of what is recorded besides it (a source of its own
/// in the settings file is more precise and wins).
const MIC_VOLUME: &str = "mic";
const CAPTURE_VOLUME: &str = "capture";

/// A volume, in percent of the sound as it is (up to twice as loud).
fn volume_row(id: &str, label: String, key: &'static str) -> Row {
    row(
        id,
        label,
        Kind::Slider {
            min: 0,
            max: 200,
            step: 5,
        },
        Box::new(move |c| {
            let v = c.video.audio.volumes.get(key).copied().unwrap_or(1.0);
            Value::Int((f64::from(v) * 100.0).round() as i64)
        }),
        Box::new(move |c, v| match v {
            Value::Int(n) => {
                let volumes = &mut c.video.audio.volumes;
                if n == 100 {
                    volumes.remove(key);
                } else {
                    volumes.insert(key.to_owned(), n as f32 / 100.0);
                }
                Ok(())
            }
            _ => Err(Invalid),
        }),
    )
}

pub fn audio(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let chosen = &config.video.audio.sources;
    let mode = capture_mode(config);
    let first_output = env.audio.outputs.first().map(|d| format!("out:{}", d.id));
    let capture = row(
        "audio_capture",
        t(Key::SetCapture),
        Kind::Choice(vec![
            opt("system", t(Key::CapSystem)),
            opt("target", t(Key::CapTarget)),
            opt("output", t(Key::CapOutput)),
            opt("programs", t(Key::CapPrograms)),
            opt("none", t(Key::CapNone)),
        ]),
        Box::new(|c| Value::Text(capture_mode(c).to_owned())),
        Box::new(move |c, v| match v {
            Value::Text(mode) => {
                set_capture(c, &mode, first_output.as_deref());
                Ok(())
            }
            _ => Err(Invalid),
        }),
    );
    let mut rows = vec![match mode {
        "target" => hinted(capture, t(Key::CapTargetHint)),
        _ => capture,
    }];
    match mode {
        // Which device: the outputs, then the inputs, each in alphabetical order.
        "output" => {
            let mut devices = Vec::new();
            for (prefix, list, title) in [
                ("out", &env.audio.outputs, Key::SrcOutputs),
                ("in", &env.audio.inputs, Key::SrcInputs),
            ] {
                let mut group: Vec<Opt> = list
                    .iter()
                    .map(|d| opt(&format!("{prefix}:{}", d.id), d.name.as_str()))
                    .collect();
                // An unplugged device that the profile records.
                for spec in chosen
                    .iter()
                    .filter(|s| s.starts_with(&format!("{prefix}:")))
                {
                    if !group.iter().any(|o| o.value == *spec) {
                        group.push(opt(
                            spec,
                            format!("{} ({})", absent_label(spec), t(Key::SrcAbsent)),
                        ));
                    }
                }
                group.sort_by_key(|o| o.label.to_lowercase());
                if !group.is_empty() {
                    devices.push(crate::heading(t(title)));
                    devices.extend(group);
                }
            }
            rows.push(choice(
                "audio_output",
                t(Key::SetAudioOutput),
                devices,
                |c| {
                    c.video
                        .audio
                        .sources
                        .iter()
                        .find(|s| is_device(s))
                        .cloned()
                        .unwrap_or_default()
                },
                |c, v| {
                    let sources = &mut c.video.audio.sources;
                    sources.retain(|s| !is_device(s));
                    sources.push(v);
                },
            ));
        }
        // The programs that play sound now, and those ticked before that are closed.
        "programs" => {
            for p in &env.audio.programs {
                rows.push(program_row(
                    format!("app:{}", p.id),
                    p.name.clone(),
                    p.icon.clone(),
                ));
            }
            for spec in chosen.iter().filter(|s| s.starts_with("app:")) {
                if !env
                    .audio
                    .programs
                    .iter()
                    .any(|p| format!("app:{}", p.id) == *spec)
                {
                    let label = format!("{} ({})", absent_label(spec), t(Key::SrcClosed));
                    rows.push(program_row(spec.clone(), label, None));
                }
            }
            if !rows.iter().any(|r| r.id.starts_with("src:app:")) {
                rows.push(info("no_programs", t(Key::SrcNoPrograms), String::new()));
            }
        }
        _ => {}
    }

    if mode != "none" {
        rows.push(volume_row(
            "capture_volume",
            t(Key::SetCaptureVolume),
            CAPTURE_VOLUME,
        ));
    }

    // ---- the microphone: a switch, then which one
    rows.push(header("h_mic"));
    rows.push(row(
        "mic_on",
        t(Key::SetMicOn),
        Kind::Toggle,
        Box::new(|c| Value::Bool(c.video.audio.sources.iter().any(|s| is_mic(s)))),
        Box::new(|c, v| match v {
            Value::Bool(on) => {
                let sources = &mut c.video.audio.sources;
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
    if chosen.iter().any(|s| is_mic(s)) {
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
        rows.push(choice(
            "mic_device",
            t(Key::SetMicDevice),
            mics,
            |c| {
                c.video
                    .audio
                    .sources
                    .iter()
                    .find(|s| is_mic(s))
                    .cloned()
                    .unwrap_or_else(|| "mic".into())
            },
            |c, v| {
                let sources = &mut c.video.audio.sources;
                sources.retain(|s| !is_mic(s));
                sources.push(v);
            },
        ));
        rows.push(crate::meter("mic_level", t(Key::SetMicLevel)));
        rows.push(volume_row("mic_volume", t(Key::SetMicVolume), MIC_VOLUME));
        rows.push(toggle(
            "audio_denoise",
            t(Key::SetAudioDenoise),
            |c| c.video.audio.mic_noise_reduction,
            |c, v| c.video.audio.mic_noise_reduction = v,
        ));
    }

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
        |c| c.video.audio.routing.clone(),
        |c, v| c.video.audio.routing = v,
    ));
    let auto = auto_audio_codec(env, config);
    rows.push(choice(
        "audio_codec",
        t(Key::SetAudioCodec),
        vec![
            crate::heading(t(Key::OptGroupCompressed)),
            opt(
                "auto",
                format!(
                    "{} ({})",
                    t(Key::OptAuto),
                    if auto == "opus" { "Opus" } else { "AAC" }
                ),
            ),
            opt("aac", "AAC"),
            opt("opus", "Opus"),
            crate::heading(t(Key::OptGroupLossless)),
            opt("flac", t(Key::OptFlac)),
            opt("pcm16", t(Key::OptPcm16)),
            opt("pcm24", t(Key::OptPcm24)),
        ],
        |c| c.video.audio.codec.clone(),
        |c, v| c.video.audio.codec = v,
    ));
    let codec = match config.video.audio.codec.as_str() {
        "auto" => auto,
        other => other,
    };
    let recommended = |label: String| format!("{label} ({})", t(Key::OptRecommended));
    // A bitrate only means something for the compressed codecs (not FLAC or PCM).
    if matches!(codec, "aac" | "opus") {
        rows.push(choice(
            "audio_bitrate",
            t(Key::SetAudioBitrate),
            AUDIO_BITRATES
                .iter()
                .map(|k| {
                    let label = format!("{k} kbit/s");
                    let label = if *k == 160 { recommended(label) } else { label };
                    opt(&k.to_string(), label)
                })
                .collect(),
            |c| audio_bitrate_option(c.video.audio.bitrate_kbps).to_string(),
            |c, v| c.video.audio.bitrate_kbps = v.parse().unwrap_or(160),
        ));
    }
    // Only Opus varies its bitrate (AAC stays constant).
    if codec == "opus" {
        rows.push(choice(
            "audio_vbr",
            t(Key::SetAudioVbr),
            vec![
                opt("vbr", recommended(t(Key::OptVbr))),
                opt("cbr", t(Key::OptCbr)),
            ],
            |c| if c.video.audio.vbr { "vbr" } else { "cbr" }.to_owned(),
            |c, v| c.video.audio.vbr = v == "vbr",
        ));
    }
    let channels = choice(
        "audio_channels",
        t(Key::SetAudioChannels),
        vec![
            opt("mono", "Mono"),
            opt("stereo", recommended(t(Key::OptStereo))),
            opt("5.1", "5.1"),
            opt("7.1", "7.1"),
        ],
        |c| c.video.audio.channels.clone(),
        |c, v| c.video.audio.channels = v,
    );
    // Surround needs Matroska and a codec that carries it: say so when it falls back to stereo.
    let surround = matches!(config.video.audio.channels.as_str(), "5.1" | "7.1");
    let carried = config.video.container == "mkv" && codec != "aac";
    rows.push(if surround && !carried {
        hinted(channels, t(Key::SetSurroundHint))
    } else {
        channels
    });

    rows
}

/// The replay buffer: the last moments, kept to be saved on demand.
pub fn replay(env: &Env, config: &Config) -> Vec<Row> {
    let t = |k| env.t(k);
    let storage = choice(
        "replay_storage",
        t(Key::SetReplayStorage),
        vec![
            opt(
                "auto",
                format!("{} ({})", t(Key::OptAuto), t(Key::OptRecommended)),
            ),
            opt("ram", t(Key::SetStorageRam)),
            opt("disk", t(Key::SetStorageDisk)),
        ],
        |c| c.replay.storage.clone(),
        |c, v| c.replay.storage = v,
    );
    let storage = match replay_estimate(env, config) {
        Some(hint) => hinted(storage, hint),
        None => storage,
    };
    let mut rows = vec![
        hinted(
            toggle(
                "replay_enabled",
                t(Key::SetReplayStart),
                |c| c.replay.enabled,
                |c, v| c.replay.enabled = v,
            ),
            t(Key::SetReplayStartHint),
        ),
        choice(
            "replay_duration",
            t(Key::SetReplayDuration),
            REPLAY_DURATIONS
                .iter()
                .map(|s| {
                    let label = if *s < 60 {
                        format!("{s} s")
                    } else {
                        format!("{} min", s / 60)
                    };
                    opt(&s.to_string(), label)
                })
                .collect(),
            |c| {
                let s = c.replay.duration_seconds;
                REPLAY_DURATIONS
                    .iter()
                    .min_by_key(|d| d.abs_diff(s))
                    .unwrap_or(&30)
                    .to_string()
            },
            |c, v| c.replay.duration_seconds = v.parse().unwrap_or(30),
        ),
        choice(
            "replay_after",
            t(Key::SetReplayAfter),
            REPLAY_AFTER
                .iter()
                .map(|s| {
                    let label = if *s == 0 {
                        t(Key::OptReplayAfterNone)
                    } else {
                        t(Key::OptReplayAfter).replace("{s}", &s.to_string())
                    };
                    opt(&s.to_string(), label)
                })
                .collect(),
            |c| {
                let s = c.replay.after_seconds;
                REPLAY_AFTER
                    .iter()
                    .min_by_key(|d| d.abs_diff(s))
                    .unwrap_or(&0)
                    .to_string()
            },
            |c, v| c.replay.after_seconds = v.parse().unwrap_or(0),
        ),
        storage,
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
    ];
    rows.extend(naming_rows(
        env,
        "replays",
        |c| &c.paths.naming.replays,
        |c| &mut c.paths.naming.replays,
    ));
    rows
}

/// How long a replay can go on after its shortcut, in seconds.
const REPLAY_AFTER: [u32; 5] = [0, 5, 10, 15, 30];

/// The durations a replay can keep, in seconds.
const REPLAY_DURATIONS: [u32; 9] = [15, 30, 60, 120, 300, 600, 900, 1200, 1800];

/// How big the replay will be with the video settings, and where `auto` keeps it. `None` while
/// the encoder is not known.
fn replay_estimate(env: &Env, config: &Config) -> Option<String> {
    let encoder = encoders::resolved(env, &config.video).and_then(|e| encoders::spec(&e.id))?;
    // The largest screen (a recording is of one screen).
    let screen = env
        .machine
        .screens
        .iter()
        .map(|s| (s.width, s.height))
        .max_by_key(|(w, h)| u64::from(*w) * u64::from(*h))
        .unwrap_or((1920, 1080));
    let (_, _, cw, ch) = validate::center_crop(screen, validate::aspect(&config.video.aspect));
    let size = validate::output_size(&config.video.resolution, (cw, ch)).unwrap_or((cw, ch));
    let bytes = replay::expected_bytes(
        replay::profile_kbps(&config.video, encoder, size),
        config.replay.duration_seconds + config.replay.after_seconds,
    );
    let key = match config.replay.storage.as_str() {
        "ram" | "disk" => Key::ReplayEstimate,
        _ if replay::fits_in_ram(bytes, env.machine.ram_bytes) => Key::ReplayEstimateRam,
        _ => Key::ReplayEstimateDisk,
    };
    Some(env.t(key).replace("{size}", &size_label(bytes, env.lang)))
}

/// A file size: megabytes below a gigabyte, gigabytes with one decimal above.
fn size_label(bytes: u64, lang: vixeeny_common::i18n::Lang) -> String {
    let fr = lang == vixeeny_common::i18n::Lang::Fr;
    if bytes < 1 << 30 {
        let mb = (bytes >> 20).max(1);
        format!("{mb} {}", if fr { "Mo" } else { "MB" })
    } else {
        let gb = format!("{:.1}", bytes as f64 / f64::from(1u32 << 30));
        let gb = if fr { gb.replace('.', ",") } else { gb };
        format!("{gb} {}", if fr { "Go" } else { "GB" })
    }
}

/// The settings of the updates page (the state of the update is drawn by the page).
pub fn updates(env: &Env, config: &Config) -> Vec<Row> {
    let mut rows = vec![toggle(
        "check_updates",
        env.t(Key::SetCheckUpdates),
        |c| c.general.check_updates,
        |c, v| c.general.check_updates = v,
    )];
    if config.general.check_updates {
        rows.push(toggle(
            "auto_update",
            env.t(Key::SetAutoUpdate),
            |c| c.general.auto_update,
            |c, v| c.general.auto_update = v,
        ));
    }
    rows
}
