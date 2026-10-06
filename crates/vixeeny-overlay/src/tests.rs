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
