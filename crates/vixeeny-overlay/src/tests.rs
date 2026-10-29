// SPDX-License-Identifier: GPL-3.0-or-later
//! Off-screen drawings of every piece. With `VIXEENY_SCREENSHOTS` set to a folder, each is also written there as a PNG
//! (over a desktop-like background), to be looked at.

use vixeeny_editor::{Color, RgbaImage};
use vixeeny_platform::PhysicalRect;

use crate::gfx::{Canvas, Gfx, rect};
use crate::layout::PanelHit;
use crate::paint::{self, BarLook, FieldLook, PanelLook};
use crate::theme::{Look, Rgba, Theme};

pub fn save(name: &str, w: u32, h: u32, bgra: &[u8]) {
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
    encoder.write_header().and_then(|mut w| w.write_image_data(&rgba)).expect("screenshot written");
}

/// A dimmed desktop-like backdrop, so translucent pieces look as they will.
fn backdrop(c: &Canvas<'_>, w: u32, h: u32) -> windows::core::Result<()> {
    c.gradient(
        rect(0.0, 0.0, w as f32, h as f32),
        0.0,
        (0.0, 0.0),
        (w as f32, h as f32),
        &[(0.0, Rgba::hex(0x3a6ea5, 255)), (1.0, Rgba::hex(0xc9d6df, 255))],
    )?;
    c.fill_rect(rect(0.0, 0.0, w as f32, h as f32), Rgba([0.0, 0.0, 0.0, 0.4]))
}

fn bar_look(upright: bool) -> BarLook {
    BarLook {
        tool: Some(vixeeny_editor::Tool::Ellipse),
        picks: crate::layout::Picks { shape: vixeeny_editor::Tool::Ellipse, ..Default::default() },
        upright,
        color: Color::PALETTE[0],
        live: true,
        can_live: true,
        can_undo: true,
        can_redo: false,
        can_clear: true,
        style_open: true,
        hover: Some(2),
        pressed: None,
    }
}

#[test]
fn every_piece_draws() {
    let gfx = Gfx::new().expect("graphics devices");
    for (name, look) in [("dark", Look::default()), ("light", Look { dark: false, ..Look::default() })] {
        let t = Theme::new(look);
        let u = 1.25;
        let (bw, bh) = paint::bar_surface(u, false);
        let (uw, uh) = paint::bar_surface(u, true);
        let (pw, ph) = paint::panel_surface(u);
        let tool = vixeeny_editor::Tool::Ellipse;
        let panel = PanelLook {
            color: Color::PALETTE[4],
            recent: vec![Color::rgb(10, 200, 120), Color::PALETTE[4]],
            width: 6.0,
            max_width: 40.0,
            size: 6.0,
            filled: true,
            tool: Some(tool),
            form: crate::layout::PanelForm::of(Some(tool), true),
            hsv: (200.0, 0.7, 0.8),
            eyedropper: false,
            hover: Some(PanelHit::Size),
        };
        let (w, h) = (bw.max(pw + uw + 20) + 40, bh + ph.max(uh) + 160);
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                let at = |x: f32, y: f32| windows_numerics::Matrix3x2::translation(x, y);
                // SAFETY: plain transform calls on the off-screen context.
                unsafe { c.dc.SetTransform(&at(10.0, 10.0)) };
                paint::panel(c, &t, u, &panel)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at((pw + 20) as f32, 10.0)) };
                paint::toolbar(c, &t, u, &bar_look(true))?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(10.0, ph.max(uh) as f32)) };
                paint::toolbar(c, &t, u, &bar_look(false))?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(440.0, (ph.max(uh) + bh) as f32)) };
                paint::brush(c, 24)?;
                let text = "Rectangle";
                let ((_, _), bubble) = paint::tip_size(&gfx, u, text)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(300.0, (ph.max(uh) + bh) as f32)) };
                paint::tip(c, &t, u, text, bubble)?;
                let label = "1280 × 720";
                let size = paint::label_size(&gfx, u, label)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(40.0, (ph.max(uh) + bh + 20) as f32)) };
                paint::size_label(c, &t, u, label, size)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(500.0, (ph.max(uh) + bh) as f32)) };
                paint::handle(c, &t, u)?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(560.0, (ph.max(uh) + bh) as f32)) };
                paint::field(
                    c,
                    &FieldLook { text: "Hello".into(), caret: 3, caret_on: true, size: 24.0, color: Color::PALETTE[2] },
                )?;
                Ok(())
            })
            .expect("drawn");
        save(&format!("pieces-{name}"), w, h, &pixels);
        // Something was drawn over the backdrop where the toolbar is.
        assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
    }
}

fn side_texts() -> crate::side::SideTexts {
    crate::side::SideTexts {
        region: "Region".into(),
        window: "Window".into(),
        screen: "Screen".into(),
        all_monitors: "All screens".into(),
        scrolling: "Scrolling capture".into(),
        record: "Record".into(),
        stop_recording: "Stop recording".into(),
        replay_start: "Start replay buffer".into(),
        replay_stop: "Stop replay buffer".into(),
        replay_save: "Save replay".into(),
        settings: "Settings".into(),
        shortcuts: vec![(crate::side::Choice::Window, "ALT + PRINT SCREEN".into())],
    }
}

#[test]
fn the_side_strip_draws_in_both_themes_and_orientations() {
    use crate::side::{self, Edge, SideState, Strip, StripLayout};
    let gfx = Gfx::new().expect("graphics devices");
    for (name, edge, dark, dpi) in [("dark-right", Edge::Right, true, 96), ("light-bottom", Edge::Bottom, false, 144)] {
        let state = SideState { recording: true, replay: true, dark, edge, animate: false, pinned: false };
        let entries = side::entries(&side_texts(), &state);
        let r = side::panel_geometry(edge, PhysicalRect::new(0, 0, 2560, 1440), dpi, &entries);
        let (w, h) = (r.width, r.height);
        let layout = StripLayout::new(edge, &entries, dpi, (w, h));
        let mut strip = Strip::new(entries.clone(), layout.clone(), true);
        strip.key(side::StripKey::Next);
        let t = Theme::new(Look { dark, ..Look::default() });
        let u = layout.u;
        let (_, tip) = strip.tip().expect("the keyboard shows the label");
        let bubble = side::tip_bubble(&gfx, u, &tip).expect("measured");
        let (tx, ty) = layout.tip_origin(strip.selected as usize, bubble);
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                let at = |x: f32, y: f32| windows_numerics::Matrix3x2::translation(x, y);
                let shadow = 24.0 * u;
                // SAFETY: plain transform calls on the off-screen context.
                unsafe { c.dc.SetTransform(&at(layout.strip.0 - shadow, layout.strip.1 - shadow)) };
                side::paint_strip(c, &t, &layout, &entries, &strip.look())?;
                // SAFETY: as above.
                unsafe { c.dc.SetTransform(&at(tx - 12.0 * u, ty - 12.0 * u)) };
                side::paint_tip(c, &t, u, &tip, bubble)
            })
            .expect("drawn");
        save(&format!("side-{name}"), w, h, &pixels);
        // The strip's fill is dark in the dark theme, light in the light one, between two icons.
        let (sx, sy, sw, sh) = layout.strip;
        let (px, py) =
            if edge.is_vertical() { (sx + sw - 3.0 * u, sy + sh / 2.0) } else { (sx + sw / 2.0, sy + sh - 3.0 * u) };
        let o = ((py as u32 * w + px as u32) * 4) as usize;
        let red = pixels[o + 2];
        assert!(if dark { red < 80 } else { red > 200 }, "{name}: {red}");
    }
}

#[test]
fn the_notification_draws_with_a_thumbnail_and_as_an_error() {
    use crate::toast::{CardLayout, CardLook, ToastContent, paint_card, paint_countdown};
    let gfx = Gfx::new().expect("graphics devices");
    let green = [[40u8, 200, 90, 255]; 64 * 48].concat();
    let icon = gfx.rgba_bitmap(16, 16, &[[120u8, 80, 230, 255]; 16 * 16].concat()).expect("icon");
    let saved = ToastContent {
        heading: "Image saved".into(),
        body: "capture-001.png".into(),
        thumb: Some(RgbaImage { width: 64, height: 48, data: green }),
        dark: true,
        action_label: "Open folder".into(),
        clickable: true,
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
        let t = Theme::new(Look { dark: content.dark, ..Look::default() });
        let thumb = content.thumb.as_ref().map(|t| gfx.rgba_bitmap(t.width, t.height, &t.data).expect("bitmap"));
        let look = CardLook { hovered: true, hover: Some(crate::toast::CardTarget::Button), held: None };
        let (w, h) = l.size;
        let (bx, by, bw, bh) = l.countdown;
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                paint_card(c, &t, content, &l, thumb.as_ref(), Some(&icon), &look)?;
                // The bar two thirds of the way through the card's life.
                let shown = bw * 2.0 / 3.0;
                c.part(rect(bx, by, shown, bh), (bx, by), |c| paint_countdown(c, &t, bw, bh))
            })
            .expect("drawn");
        save(&format!("toast-{name}"), w, h, &pixels);
        // The text fits: everything is inside the card, under the title line and the bar across the card.
        let (cx, cy, cw, ch) = l.card;
        let (_, (_, text_y)) = &l.body;
        let bottom = l.button.as_ref().map_or(text_y + l.body.0.height, |(_, b)| b.1 + b.3);
        assert!(bottom <= cy + ch, "{name}: {bottom} > {}", cy + ch);
        assert!(l.app_icon.1 + l.app_icon.3 <= by && by + bh <= l.heading.1.1, "{name}: title line, bar, text");
        assert!(bx == cx && bw == cw, "{name}: the bar spans the card");
        let (ix, iy, iw, ih) = l.app_icon;
        let o = (((iy + ih / 2.0) as u32 * w + (ix + iw / 2.0) as u32) * 4) as usize;
        assert_eq!(&pixels[o..o + 3], &[230, 80, 120], "{name}: the app's icon is drawn");
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
    use crate::widget::{Alert, AlertTip, Side, WidgetLook, WidgetTarget, bar_width, paint_alert_tip, paint_window};
    use crate::widget::{format_elapsed, widget_surface};
    let gfx = Gfx::new().expect("graphics devices");
    let mut drawn = Vec::new();
    let mut widths = Vec::new();
    for (name, dark, paused, alert, secs, hover) in [
        ("recording", true, false, Alert::None, 5, None),
        ("paused", true, true, Alert::None, 754, None),
        ("hover-pause", false, false, Alert::None, 5, Some(WidgetTarget::State)),
        ("hover-resume", true, true, Alert::None, 5, Some(WidgetTarget::State)),
        ("warning", false, false, Alert::Warning, 3_725, Some(WidgetTarget::Stop)),
        ("critical", true, true, Alert::Critical, 3_725, None),
    ] {
        let u = 1.25;
        let t = Theme::new(Look::new(dark, None, true));
        let time = format_elapsed(std::time::Duration::from_secs(secs));
        let width = bar_width(&gfx, u, &time).expect("measured");
        widths.push(width);
        // The warning sign on the right of a widget on the left, on the left of one on the right.
        let sign_at = match alert {
            Alert::None => None,
            Alert::Warning => Some(Side::Right),
            Alert::Critical => Some(Side::Left),
        };
        let look = WidgetLook { paused, time, pulse: true, alert, hover, held: None, width, sign_at };
        let (w, h) = widget_surface(u, &look);
        let pixels = gfx
            .render(w, h, |c| {
                backdrop(c, w, h)?;
                paint_window(c, &t, u, &look)
            })
            .expect("drawn");
        save(&format!("widget-{name}"), w, h, &pixels);
        drawn.push(pixels);
    }
    // Red dot vs. pause sign vs. what a click does; the bar as wide as what it shows.
    assert_ne!(drawn[0], drawn[1]);
    assert_ne!(drawn[0], drawn[2]);
    assert!(widths[0] < widths[1] && widths[1] < widths[4] && widths[4] < 160, "{widths:?}");
    assert!(drawn[0].as_chunks::<4>().0.iter().any(|p| p[2] > 200 && p[1] < 80));
    let tip = AlertTip::new(&gfx, 1.25, "42 frames lost: the encoder cannot keep up\nDisk almost full: 3.1 GB free")
        .expect("laid out");
    assert_eq!(tip.text.lines, 2);
    let (w, h) = tip.size;
    let pixels = gfx
        .render(w, h, |c| {
            backdrop(c, w, h)?;
            paint_alert_tip(c, &Theme::new(Look::default()), 1.25, &tip)
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
    let cut = gfx.paragraph_lines(&text, 12.0, false, 250.0, 4).expect("laid out");
    assert_eq!(cut.lines, 4);
    assert!(cut.height < whole.height / 4.0);
    let short = gfx.paragraph_lines("a few words", 12.0, false, 250.0, 4).expect("laid out");
    assert_eq!(short.lines, 1);
}

#[test]
fn a_dragged_window_stays_whole_on_its_screen() {
    use crate::popup::keep_inside;
    use windows::Win32::Foundation::RECT;
    let work = RECT { left: 0, top: 0, right: 1920, bottom: 1032 };
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
        thumb: Some(RgbaImage { width: w, height: h, data: blue }),
        heading: "Image saved".into(),
        dark: true,
        picture_only: true,
        menu: vec!["Open".into(), "Show".into(), "Close".into()],
        ..ToastContent::default()
    };
    let l = CardLayout::new(&gfx, &content, 120).expect("laid out");
    let (_, _, cw, ch) = l.card;
    // As large as fits 320×200 at 125 %, with the picture's shape, under its title line.
    let (_, _, pw, ph) = l.picture;
    assert!((pw / l.u - 320.0).abs() < 1.0 && (ph / l.u - 180.0).abs() < 1.0, "{pw}×{ph}");
    assert!((cw - pw).abs() < 1.0 && ch > ph, "{cw}×{ch}");
    let (cx, cy, _, _) = l.card;
    assert_eq!(l.at(cx + cw - 3.0, cy + 3.0, true), CardTarget::Card, "no close button");
    let t = Theme::new(Look { dark: true, ..Look::default() });
    let bitmap = gfx.rgba_bitmap(w, h, content.thumb.as_ref().unwrap().data.as_slice());
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
                None,
                &CardLook { hovered: true, hover: Some(CardTarget::Card), held: None },
            )
        })
        .expect("drawn");
    save("toast-picture", sw, sh, &pixels);
    let o = (((cy + ch / 2.0) as u32 * sw + (cx + cw / 2.0) as u32) * 4) as usize;
    assert_eq!(&pixels[o..o + 3], &[40, 120, 200], "the picture fills the card");
}

/// A picture with detail everywhere, for the effects to show.
fn screens(w: u32, h: u32) -> RgbaImage {
    let data = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let check = if (x / 6 + y / 6) % 2 == 0 { 40 } else { 0 };
            [(x * 255 / w) as u8, (y * 255 / h) as u8, 150 + check as u8, 255]
        })
        .collect();
    RgbaImage::new(w, h, data).expect("image")
}

fn every_annotation() -> Vec<vixeeny_editor::Annotation> {
    use vixeeny_editor::{Annotation, Point, Rect, Style};
    let style = |c: Color, width: f32| Style { color: c, width };
    let p = Point::new;
    vec![
        Annotation::Pen {
            points: vec![p(10.0, 10.0), p(40.0, 30.0), p(70.0, 12.0), p(90.0, 40.0)],
            style: style(Color::rgb(230, 30, 30), 5.0),
        },
        Annotation::Rect {
            rect: Rect::new(20.0, 50.0, 60.0, 40.0),
            style: style(Color::rgb(30, 160, 60), 4.0),
            filled: false,
        },
        Annotation::Ellipse {
            rect: Rect::new(100.0, 10.0, 50.0, 30.0),
            style: style(Color::rgb(250, 200, 0), 3.0),
            filled: true,
        },
        Annotation::Arrow { from: p(110.0, 100.0), to: p(180.0, 60.0), style: style(Color::rgb(20, 20, 20), 4.0) },
        Annotation::Highlight {
            points: vec![p(10.0, 105.0), p(90.0, 105.0)],
            color: Color { a: 128, ..Color::rgb(255, 230, 0) },
            width: 14.0,
        },
        Annotation::Blur { rect: Rect::new(150.0, 80.0, 45.0, 35.0), radius: 6.0 },
        Annotation::Pixelate { rect: Rect::new(160.0, 5.0, 36.0, 36.0), block: 8 },
        Annotation::Text {
            pos: p(30.0, 60.0),
            text: "Hé".into(),
            size: 18.0,
            color: Color::rgb(255, 255, 255),
            background: Some(Color::rgb(40, 40, 120)),
        },
        Annotation::Marker { center: p(130.0, 70.0), number: 7, color: Color::rgb(220, 40, 40), radius: 11.0 },
    ]
}

#[test]
fn the_graphics_card_draws_the_annotations_as_the_export_does() {
    use crate::drawing::Drawing;
    let gfx = Gfx::new().expect("graphics devices");
    let (w, h) = (200, 120);
    let base = screens(w, h);
    let layers = std::rc::Rc::new(every_annotation());
    let mut drawing = Drawing::new(&gfx, (0, 0, w, h), &base, 0).expect("drawing");
    drawing.update(&gfx, &layers, false).expect("drawn");
    let gpu = gfx
        .render(w, h, |c| {
            drawing.draw(c, (0.0, 0.0), false);
            Ok(())
        })
        .expect("rendered");
    save("drawing-gpu", w, h, &gpu);
    let cpu = vixeeny_editor::render::render_region(
        &base,
        layers.iter(),
        &vixeeny_editor::Rect::new(0.0, 0.0, w as f32, h as f32),
    )
    .expect("exported");
    let cpu_bgra: Vec<u8> = cpu.data.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    save("drawing-export", w, h, &cpu_bgra);
    // The same picture, but for anti-aliasing and the effects' filters.
    let mut total = 0u64;
    let mut far = 0;
    let pixels = gpu.as_chunks::<4>().0.iter().zip(cpu_bgra.as_chunks::<4>().0);
    for (g, c) in pixels {
        let d = (0..3).map(|i| u64::from(g[i].abs_diff(c[i]))).max().unwrap_or(0);
        total += d;
        if d > 60 {
            far += 1;
        }
    }
    let n = u64::from(w * h);
    assert!(total / n < 6, "mean difference {}", total / n);
    assert!(far * 100 < n * 3, "{far} pixels far apart");
}

#[test]
fn live_only_what_is_drawn_covers_the_screens() {
    use crate::drawing::Drawing;
    use vixeeny_editor::{Annotation, Rect, Style};
    let gfx = Gfx::new().expect("graphics devices");
    let (w, h) = (100, 60);
    let layers = std::rc::Rc::new(vec![
        Annotation::Rect {
            rect: Rect::new(10.0, 10.0, 40.0, 30.0),
            style: Style { color: Color::rgb(255, 0, 0), width: 4.0 },
            filled: false,
        },
        Annotation::Blur { rect: Rect::new(60.0, 10.0, 30.0, 30.0), radius: 4.0 },
    ]);
    let mut drawing = Drawing::new(&gfx, (0, 0, w, h), &screens(w, h), 0).expect("drawing");
    drawing.update(&gfx, &layers, true).expect("drawn");
    let px = gfx
        .render(w, h, |c| {
            drawing.draw(c, (0.0, 0.0), true);
            Ok(())
        })
        .expect("rendered");
    save("drawing-live", w, h, &px);
    let alpha = |x: usize, y: usize| px[(y * w as usize + x) * 4 + 3];
    eprintln!("{:?}", (0..100).step_by(5).map(|x| alpha(x, 25)).collect::<Vec<_>>());
    // The rectangle's edge and the blur cover the screens; inside the outline, they show.
    assert_eq!(alpha(10, 25), 255);
    assert_eq!(alpha(75, 25), 255);
    assert_eq!(alpha(30, 25), 0);
    assert_eq!(alpha(5, 55), 0);
}

#[test]
fn live_pictures_are_copied_in_on_the_graphics_card() {
    use crate::drawing::{Drawing, ScreenPart};
    use vixeeny_editor::{Annotation, Point, Style};
    use windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    let gfx = Gfx::new().expect("graphics devices");
    let (w, h) = (100, 60);
    // A monitor's picture, all one colour (BGRA).
    let (tw, th) = (50u32, 40u32);
    let pixels: Vec<u8> = [10u8, 200, 30, 255].repeat((tw * th) as usize);
    let texture = vixeeny_platform::d3d::texture(
        &gfx.d3d,
        (tw, th),
        DXGI_FORMAT_B8G8R8A8_UNORM,
        D3D11_BIND_SHADER_RESOURCE.0 as u32,
        0,
        Some((&pixels, tw * 4)),
    );
    let part = ScreenPart {
        x: 20,
        y: 10,
        width: 30,
        height: 20,
        texture: texture.expect("texture"),
        from: (5, 5),
        keep: Box::new(()),
    };
    let layers = std::rc::Rc::new(vec![Annotation::Line {
        from: Point::new(90.0, 50.0),
        to: Point::new(95.0, 55.0),
        style: Style { color: Color::rgb(255, 0, 0), width: 2.0 },
    }]);
    let base = screens(w, h);
    let mut drawing = Drawing::new(&gfx, (0, 0, w, h), &base, 0).expect("drawing");
    drawing.feed(&gfx, &part).expect("fed");
    drawing.update(&gfx, &layers, false).expect("drawn");
    let px = gfx
        .render(w, h, |c| {
            drawing.draw(c, (0.0, 0.0), false);
            Ok(())
        })
        .expect("rendered");
    let at = |x: usize, y: usize| &px[(y * w as usize + x) * 4..(y * w as usize + x) * 4 + 3];
    // The part fed shows where it goes, the screens taken before elsewhere (as BGRA).
    assert_eq!(at(25, 15), [10, 200, 30]);
    assert_eq!(at(49, 29), [10, 200, 30]);
    let o = (5 * w as usize + 5) * 4;
    assert_eq!(at(5, 5), [base.data[o + 2], base.data[o + 1], base.data[o]]);
    assert_ne!(at(50, 30), [10, 200, 30]);
}

#[test]
fn a_drawing_made_anew_is_never_taken_for_the_one_before() {
    use crate::drawing::Drawing;
    use vixeeny_editor::{Annotation, Color, Rect, Style};
    let gfx = Gfx::new().expect("graphics devices");
    let (w, h) = (64, 64);
    let layers = std::rc::Rc::new(vec![Annotation::Rect {
        rect: Rect::new(4.0, 4.0, 20.0, 20.0),
        style: Style { color: Color::rgb(255, 0, 0), width: 2.0 },
        filled: false,
    }]);
    // The zone moved: the same annotations, a new area, a new drawing. The windows show a drawing again only when its
    // version changed.
    let mut before = Drawing::new(&gfx, (0, 0, w, h), &screens(w, h), 0).expect("drawing");
    before.update(&gfx, &layers, false).expect("drawn");
    let mut after = Drawing::new(&gfx, (8, 0, w - 8, h), &screens(w, h), 0).expect("drawing");
    after.update(&gfx, &layers, false).expect("drawn");
    assert_ne!(before.version, after.version);
}

/// The editor keeps the size label clear of the toolbar with a size of its own: the label drawn is never larger.
#[test]
fn the_size_label_fits_the_size_the_editor_places() {
    let gfx = Gfx::new().expect("graphics devices");
    let (w, h) = vixeeny_editor::session::LABEL_SIZE;
    for u in [1.0, 1.5, 2.0] {
        let (lw, lh) = paint::label_size(&gfx, u, "9999 × 9999").unwrap();
        assert!(lw as f32 <= w * u && lh as f32 <= h * u, "{lw} × {lh} at {u}");
    }
}
