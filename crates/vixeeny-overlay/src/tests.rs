// SPDX-License-Identifier: GPL-3.0-or-later
//! Off-screen drawings of every piece. With `VIXEENY_SCREENSHOTS` set to a folder, each is also
//! written there as a PNG (over a desktop-like background), to be looked at.

use vixeeny_editor::{Color, MagnifierView, RgbaImage};

use crate::gfx::{Canvas, Gfx, rect};
use crate::layout::PanelHit;
use crate::paint::{self, BarLook, FieldLook, PanelLook};
use crate::theme::{Look, Rgba, Theme};

fn save(name: &str, w: u32, h: u32, bgra: &[u8]) {
    let Some(dir) = std::env::var_os("VIXEENY_SCREENSHOTS") else {
        return;
    };
    let mut rgba = Vec::with_capacity(bgra.len());
    for px in bgra.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    let path = std::path::Path::new(&dir).join(format!("overlay-{name}.png"));
    let file = std::fs::File::create(&path).expect("screenshot file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&rgba))
        .expect("screenshot written");
}

/// A dimmed desktop-like backdrop, so translucent pieces look as they will.
fn backdrop(c: &Canvas<'_>, w: u32, h: u32) -> windows::core::Result<()> {
    c.gradient(
        rect(0.0, 0.0, w as f32, h as f32),
        0.0,
        (0.0, 0.0),
        (w as f32, h as f32),
        &[
            (0.0, Rgba::hex(0x3a6ea5, 255)),
            (1.0, Rgba::hex(0xc9d6df, 255)),
        ],
    )?;
    c.fill_rect(
        rect(0.0, 0.0, w as f32, h as f32),
        Rgba([0.0, 0.0, 0.0, 0.4]),
    )
}

fn bar_look() -> BarLook {
    BarLook {
        tool: 1,
        color: Color::PALETTE[0],
        can_undo: true,
        can_redo: false,
        style_open: false,
        hover: Some(4),
        pressed: None,
    }
}

#[test]
fn every_piece_draws() {
    let gfx = Gfx::new().expect("graphics devices");
    for (name, look) in [
        ("dark", Look::default()),
        (
            "light",
            Look {
                dark: false,
                ..Look::default()
            },
        ),
    ] {
        let t = Theme::new(look);
        let u = 1.25;
        let (bw, bh) = paint::bar_surface(u);
        let (pw, ph) = paint::panel_surface(u);
        let panel = PanelLook {
            color: Color::PALETTE[4],
            recent: vec![Color::rgb(10, 200, 120), Color::PALETTE[4]],
            width: 4.0,
            filled: true,
            picker: true,
            hsv: (200.0, 0.7, 0.8),
            eyedropper: false,
            hover: Some(PanelHit::Thicker),
        };
        let (w, h) = (bw.max(pw) + 40, bh + ph + 160);
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                let at = |x: f32, y: f32| windows_numerics::Matrix3x2::translation(x, y);
                // SAFETY: plain transform calls on the off-screen context.
                unsafe { c.dc.SetTransform(&at(10.0, 10.0)) };
                paint::panel(c, &t, u, &panel)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(10.0, ph as f32)) };
                paint::toolbar(c, &t, u, &bar_look())?;
                let text = "Rectangle";
                let ((_, _), bubble) = paint::tip_size(&gfx, u, text)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(300.0, (ph + bh) as f32)) };
                paint::tip(c, &t, u, text, bubble)?;
                let label = "1280 × 720";
                let size = paint::label_size(&gfx, u, label)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(40.0, (ph + bh + 20) as f32)) };
                paint::size_label(c, &t, u, label, size)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(500.0, (ph + bh) as f32)) };
                paint::handle(c, &t, u)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(560.0, (ph + bh) as f32)) };
                paint::field(
                    c,
                    &FieldLook {
                        text: "Hello".into(),
                        caret: 3,
                        caret_on: true,
                        size: 24.0,
                        color: Color::PALETTE[2],
                    },
                )?;
                Ok(())
            })
            .expect("drawn");
        save(&format!("pieces-{name}"), w, h, &pixels);
        // Something was drawn over the backdrop where the toolbar is.
        assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
    }
}

#[test]
fn the_magnifier_draws_its_pixels() {
    let gfx = Gfx::new().expect("graphics devices");
    let t = Theme::new(Look::default());
    let u = 1.0;
    let side = vixeeny_editor::session::MAGNIFIER_SIDE;
    let mut data = Vec::new();
    for y in 0..side {
        for x in 0..side {
            data.extend_from_slice(&[(x * 23) as u8, (y * 23) as u8, 128, 255]);
        }
    }
    let m = MagnifierView {
        pixels: RgbaImage::new(side, side, data).expect("image"),
        position: Default::default(),
        size: 110.0,
        hex: "#5c3a80".into(),
        color: Color::rgb(0x5c, 0x3a, 0x80),
    };
    let (w, h) = paint::magnifier_size(u, &m);
    let pixels = gfx
        .render(w, h, |c| paint::magnifier(c, &t, u, &m))
        .expect("drawn");
    save("magnifier", w, h, &pixels);
    // The top-left source pixel (0, 0, 128) fills the first cell, 10 px wide.
    let at = |x: u32, y: u32| {
        let o = ((y * w + x) * 4) as usize;
        [pixels[o], pixels[o + 1], pixels[o + 2]]
    };
    assert_eq!(at(5, 5), [128, 0, 0]);
    // The second cell is (23, 0, 128).
    assert_eq!(at(15, 5), [128, 0, 23]);
}

fn side_texts() -> crate::side::SideTexts {
    crate::side::SideTexts {
        image: "Screenshot".into(),
        region: "Region".into(),
        window: "Window".into(),
        screen: "Screen".into(),
        all_monitors: "All screens".into(),
        scrolling: "Scrolling capture".into(),
        video: "Video".into(),
        record: "Record".into(),
        stop_recording: "Stop recording".into(),
        replay_start: "Start replay buffer".into(),
        replay_stop: "Stop replay buffer".into(),
        replay_save: "Save replay".into(),
        settings: "Settings".into(),
    }
}

/// The strip as the window shows it: the strip and its label over a desktop.
#[test]
fn the_side_strip_draws_in_both_themes_and_orientations() {
    use crate::side::{self, Edge, SideState, Strip, StripLayout};
    let gfx = Gfx::new().expect("graphics devices");
    for (name, edge, dark, dpi) in [
        ("dark-right", Edge::Right, true, 96),
        ("light-bottom", Edge::Bottom, false, 144),
    ] {
        let state = SideState {
            recording: true,
            replay: true,
            dark,
            edge,
            animate: false,
            pinned: false,
        };
        let entries = side::entries(&side_texts(), &state);
        let (_, _, w, h) = side::panel_geometry(edge, (0, 0, 2560, 1440), dpi, &entries);
        let layout = StripLayout::new(edge, &entries, dpi, (w, h));
        let mut strip = Strip::new(entries.clone(), layout.clone(), true);
        strip.key(side::StripKey::Next);
        let t = Theme::new(Look {
            dark,
            ..Look::default()
        });
        let u = layout.u;
        let (_, text) = strip.tip().expect("the keyboard shows the label");
        let bubble = side::tip_bubble(&gfx, u, text).expect("measured");
        let (tx, ty) = layout.tip_origin(strip.selected as usize, bubble);
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                let at = |x: f32, y: f32| windows_numerics::Matrix3x2::translation(x, y);
                let shadow = 24.0 * u;
                // SAFETY: plain transform calls on the off-screen context.
                unsafe {
                    c.dc.SetTransform(&at(layout.strip.0 - shadow, layout.strip.1 - shadow))
                };
                side::paint_strip(c, &t, &layout, &entries, &strip.look())?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(tx - 12.0 * u, ty - 12.0 * u)) };
                side::paint_tip(c, &t, u, text, bubble)
            })
            .expect("drawn");
        save(&format!("side-{name}"), w, h, &pixels);
        // The strip's fill is dark in the dark theme, light in the light one, between two icons.
        let (sx, sy, sw, sh) = layout.strip;
        let (px, py) = if edge.is_vertical() {
            (sx + sw - 3.0 * u, sy + sh / 2.0)
        } else {
            (sx + sw / 2.0, sy + sh - 3.0 * u)
        };
        let o = ((py as u32 * w + px as u32) * 4) as usize;
        let red = pixels[o + 2];
        assert!(if dark { red < 80 } else { red > 200 }, "{name}: {red}");
    }
}

#[test]
fn the_notification_draws_with_a_thumbnail_and_as_an_error() {
    use crate::toast::{CardLayout, CardLook, ToastContent, paint_card};
    let gfx = Gfx::new().expect("graphics devices");
    let green = [[40u8, 200, 90, 255]; 64 * 48].concat();
    let saved = ToastContent {
        heading: "Image saved".into(),
        body: "capture-001.png".into(),
        thumb: Some((64, 48, green)),
        dark: true,
        action_label: "Open folder".into(),
        ..ToastContent::default()
    };
    let failed = ToastContent {
        heading: "The capture failed".into(),
        body: "The disk is full: free some space or choose another folder in the settings, \
               then try again."
            .into(),
        error: true,
        dark: false,
        action_label: "Open settings".into(),
        ..ToastContent::default()
    };
    let mut sizes = Vec::new();
    for (name, content, dpi) in [("saved", &saved, 96), ("error", &failed, 120)] {
        let l = CardLayout::new(&gfx, content, dpi).expect("laid out");
        let t = Theme::new(Look {
            dark: content.dark,
            ..Look::default()
        });
        let thumb = content
            .thumb
            .as_ref()
            .map(|(w, h, rgba)| gfx.rgba_bitmap(*w, *h, rgba).expect("bitmap"));
        let look = CardLook {
            hovered: true,
            hover: Some(crate::toast::CardTarget::Button),
            held: None,
        };
        let (w, h) = l.size;
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                paint_card(c, &t, content, &l, thumb.as_ref(), &look)
            })
            .expect("drawn");
        save(&format!("toast-{name}"), w, h, &pixels);
        // The text fits: everything is inside the card.
        let (_, cy, _, ch) = l.card;
        let (_, (_, by)) = &l.body;
        let bottom = l
            .button
            .as_ref()
            .map_or(by + l.body.0.height, |(_, b)| b.1 + b.3);
        assert!(bottom <= cy + ch, "{name}: {bottom} > {}", cy + ch);
        if content.thumb.is_some() {
            let (px, py, pw, ph) = l.picture;
            let o = (((py + ph / 2.0) as u32 * w + (px + pw / 2.0) as u32) * 4) as usize;
            assert_eq!(&pixels[o..o + 3], &[90, 200, 40], "the thumbnail is drawn");
        }
        sizes.push(l.size.1 as f32 / l.u);
    }
    assert!(sizes[1] > sizes[0], "the longer text makes a taller card");
}

#[test]
fn the_recording_widget_draws_its_states_and_alerts() {
    use crate::widget::widget_surface;
    use crate::widget::{Alert, AlertTip, WidgetLook, WidgetTarget, paint_alert_tip, paint_widget};
    let gfx = Gfx::new().expect("graphics devices");
    let t = Theme::new(Look::default());
    let mut drawn = Vec::new();
    for (name, paused, alert) in [
        ("recording", false, Alert::None),
        ("paused", true, Alert::None),
        ("warning", false, Alert::Warning),
        ("critical", true, Alert::Critical),
    ] {
        let u = 1.25;
        let (w, h) = widget_surface(u);
        let look = WidgetLook {
            paused,
            time: "01:02:05".into(),
            pulse: true,
            alert,
            hover: Some(WidgetTarget::Stop),
            held: None,
        };
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                paint_widget(c, &t, u, &look)
            })
            .expect("drawn");
        save(&format!("widget-{name}"), w, h, &pixels);
        drawn.push(pixels);
    }
    // Red dot vs. pause sign; with and without the warning sign.
    assert_ne!(drawn[0], drawn[1]);
    assert_ne!(drawn[0], drawn[2]);
    assert!(
        drawn[0]
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[2] > 200 && p[1] < 80)
    );
    let tip = AlertTip::new(
        &gfx,
        1.25,
        "42 frame(s) lost: the encoder cannot keep up\nDisk almost full: 3.1 GB free",
    )
    .expect("laid out");
    assert_eq!(tip.text.lines, 2);
    let (w, h) = tip.size;
    let pixels = gfx
        .render(w, h, |c| {
            backdrop(c, w, h)?;
            paint_alert_tip(c, &t, 1.25, &tip)
        })
        .expect("drawn");
    save("widget-tip", w, h, &pixels);
}

#[test]
fn a_long_notification_text_is_cut_on_its_last_line() {
    let gfx = Gfx::new().expect("graphics devices");
    let text = "word ".repeat(400);
    let whole = gfx.paragraph(&text, 12.0, false, 250.0).expect("laid out");
    assert!(whole.lines > 4);
    let cut = gfx
        .paragraph_lines(&text, 12.0, false, 250.0, 4)
        .expect("laid out");
    assert_eq!(cut.lines, 4);
    assert!(cut.height < whole.height / 4.0);
    let short = gfx
        .paragraph_lines("a few words", 12.0, false, 250.0, 4)
        .expect("laid out");
    assert_eq!(short.lines, 1);
}

#[test]
fn a_dragged_window_stays_whole_on_its_screen() {
    use crate::popup::keep_inside;
    use windows::Win32::Foundation::RECT;
    let work = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1032,
    };
    assert_eq!(keep_inside((100, 200), (300, 60), work), (100, 200));
    assert_eq!(keep_inside((1800, 1000), (300, 60), work), (1620, 972));
    assert_eq!(keep_inside((-50, -9), (300, 60), work), (0, 0));
    // Wider than the screen: pinned to its left edge.
    assert_eq!(keep_inside((500, 500), (4000, 60), work), (0, 500));
}

#[test]
fn a_screenshot_notification_is_its_picture_alone() {
    use crate::toast::{CardLayout, CardLook, CardTarget, ToastContent, paint_card};
    let gfx = Gfx::new().expect("graphics devices");
    // A wide screenshot (16:9) as its 640-pixel thumbnail.
    let (w, h) = (640u32, 360u32);
    let blue = [[200u8, 120, 40, 255]; 640 * 360].concat();
    let content = ToastContent {
        thumb: Some((w, h, blue)),
        dark: true,
        picture_only: true,
        menu: vec!["Open".into(), "Show".into(), "Close".into()],
        ..ToastContent::default()
    };
    let l = CardLayout::new(&gfx, &content, 120).expect("laid out");
    let (_, _, cw, ch) = l.card;
    // As large as fits 320×200 at 125 %, with the picture's shape.
    assert!(
        (cw / l.u - 320.0).abs() < 1.0 && (ch / l.u - 180.0).abs() < 1.0,
        "{cw}×{ch}"
    );
    let (cx, cy, _, _) = l.card;
    assert_eq!(
        l.at(cx + cw - 3.0, cy + 3.0, true),
        CardTarget::Card,
        "no close button"
    );
    let t = Theme::new(Look {
        dark: true,
        ..Look::default()
    });
    let bitmap = gfx.rgba_bitmap(w, h, content.thumb.as_ref().unwrap().2.as_slice());
    let bitmap = bitmap.expect("bitmap");
    let (sw, sh) = l.size;
    let pixels = gfx
        .render(sw, sh, |c| {
            backdrop(c, sw, sh)?;
            paint_card(
                c,
                &t,
                &content,
                &l,
                Some(&bitmap),
                &CardLook {
                    hovered: true,
                    hover: Some(CardTarget::Card),
                    held: None,
                },
            )
        })
        .expect("drawn");
    save("toast-picture", sw, sh, &pixels);
    let o = (((cy + ch / 2.0) as u32 * sw + (cx + cw / 2.0) as u32) * 4) as usize;
    assert_eq!(
        &pixels[o..o + 3],
        &[40, 120, 200],
        "the picture fills the card"
    );
}
