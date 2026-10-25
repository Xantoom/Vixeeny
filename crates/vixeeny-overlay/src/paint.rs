// SPDX-License-Identifier: GPL-3.0-or-later
//! What the drawn pieces of the editor look like: the toolbar, its tips, the style panel, the size label, a handle of
//! the zone and the text field. Each piece is drawn whole into its own surface, only when it changes.

use vixeeny_editor::{Color, Tool};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::core::Result;

use crate::gfx::{Align, Canvas, Font, Gfx, rect};
use crate::icons;
use crate::layout::{self, BUTTON, Button, Kit, PanelForm, PanelHit, Picks, SHADOW};
use crate::theme::{Rgba, Theme, hsv_to_rgb};

pub fn rgba(c: Color) -> Rgba {
    Rgba([f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0, f32::from(c.a) / 255.0])
}

/// What the toolbar shows; it is drawn again only when this changes.
#[derive(Debug, Clone, PartialEq)]
pub struct BarLook {
    /// The tool in hand (`None`: the zone).
    pub tool: Option<Tool>,
    /// What the two-tool buttons take (their icon shows it).
    pub picks: Picks,
    pub upright: bool,
    pub color: Color,
    /// The screens are shown live; `can_live`: they can be.
    pub live: bool,
    pub can_live: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// There are annotations to clear.
    pub can_clear: bool,
    pub style_open: bool,
    /// Button under the pointer, and the one being pressed.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
}

/// Size in pixels of the toolbar's surface (the bar and room for its shadow).
pub fn bar_surface(u: f32, upright: bool) -> (u32, u32) {
    let (w, h) = layout::bar_size(upright);
    ((w * u + 2.0 * SHADOW * u).ceil() as u32, (h * u + 2.0 * SHADOW * u).ceil() as u32)
}

struct ButtonLook {
    active: bool,
    enabled: bool,
    hover: bool,
    pressed: bool,
}

/// The background of a 30×30 button at `(x, y)`, by its state.
fn button_back(c: &Canvas<'_>, t: &Theme, u: f32, x: f32, y: f32, b: &ButtonLook) -> Result<()> {
    let fill = if b.active {
        Some(t.accent_soft)
    } else if b.pressed && b.enabled {
        Some(t.subtle_pressed)
    } else if b.hover && b.enabled {
        Some(t.subtle_hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        c.fill_round(rect(x, y, BUTTON * u, BUTTON * u), 6.0 * u, fill)?;
    }
    Ok(())
}

/// An icon button: background, icon, and the mark of the active tool.
fn icon_button(
    c: &Canvas<'_>,
    t: &Theme,
    u: f32,
    (x, y): (f32, f32),
    icon: &'static str,
    b: &ButtonLook,
) -> Result<()> {
    button_back(c, t, u, x, y, b)?;
    if b.active {
        // The mark of the selected item, as Windows 11 has.
        c.fill_round(rect(x + 8.0 * u, y + BUTTON * u - 3.0 * u, BUTTON * u - 16.0 * u, 2.0 * u), 1.0 * u, t.accent)?;
    }
    let tint = if !b.enabled {
        t.text_off
    } else if b.active {
        t.accent
    } else {
        t.text
    };
    let size = 18.0 * u;
    let pad = (BUTTON * u - size) / 2.0;
    c.icon(icon, x + pad, y + pad, size, tint)
}

/// The box of a floating piece (toolbar, panel): shadow, translucent fill, hairline border.
fn floating_box(c: &Canvas<'_>, t: &Theme, r: D2D_RECT_F, radius: f32, alpha: f32, shadow: Rgba) -> Result<()> {
    c.shadow(r, radius, 18.0, 6.0, shadow)?;
    c.fill_round(r, radius, t.flyout.with_alpha(alpha))?;
    c.stroke_round(r, radius, 1.0, t.stroke_strong)
}

pub fn toolbar(c: &Canvas<'_>, t: &Theme, u: f32, look: &BarLook) -> Result<()> {
    let (ox, oy) = (SHADOW * u, SHADOW * u);
    let (w, h) = layout::bar_size(look.upright);
    let bar = rect(ox, oy, w * u, h * u);
    floating_box(c, t, bar, 12.0 * u, 0.94, Rgba::hex(0x000000, 0x60))?;
    for along in layout::SEPARATORS {
        let line = if look.upright {
            rect(ox + 11.0 * u, oy + along * u, 24.0 * u, 1.0)
        } else {
            rect(ox + along * u, oy + 11.0 * u, 1.0, 24.0 * u)
        };
        c.fill_rect(line, t.divider)?;
    }
    for (k, spec) in layout::buttons().enumerate() {
        let (x, y) = layout::button_xy(k, look.upright);
        let at = (ox + x * u, oy + y * u);
        let b = button_look(look, k, spec.button);
        let icon = match spec.button {
            Button::Tool(kit) => layout::tool_icon(look.picks.tool(kit)),
            Button::Style => {
                // The current colour, as a dot.
                button_back(c, t, u, at.0, at.1, &b)?;
                let (cx, cy) = (at.0 + 15.0 * u, at.1 + 15.0 * u);
                c.fill_circle(cx, cy, 9.0 * u, rgba(look.color))?;
                c.stroke_circle(cx, cy, 9.0 * u, 1.0, t.stroke_strong)?;
                continue;
            }
            // Live: what the button does now (pause the screens, or let them play).
            Button::Live if look.live => icons::PAUSE_FILLED,
            Button::Live => icons::PLAY_FILLED,
            Button::Undo => icons::UNDO,
            Button::Redo => icons::REDO,
            Button::Clear => icons::DELETE,
            Button::Copy => icons::COPY,
            Button::CopyText => icons::SCAN_TEXT,
            Button::Save => icons::SAVE,
            Button::Close => icons::DISMISS,
        };
        icon_button(c, t, u, at, icon, &b)?;
    }
    Ok(())
}

fn button_look(look: &BarLook, k: usize, button: Button) -> ButtonLook {
    ButtonLook {
        active: match button {
            Button::Tool(kit) => Kit::of(look.tool) == Some(kit),
            Button::Live => look.live,
            Button::Style => look.style_open,
            _ => false,
        },
        enabled: match button {
            Button::Live => look.can_live,
            Button::Undo => look.can_undo,
            Button::Redo => look.can_redo,
            Button::Clear => look.can_clear,
            _ => true,
        },
        hover: look.hover == Some(k),
        pressed: look.pressed == Some(k),
    }
}

/// Toolbar button `k` lights under the pointer: it can be used, and is neither the one in hand nor pressed.
pub fn bar_lights(look: &BarLook, k: usize) -> bool {
    layout::buttons().nth(k).is_some_and(|spec| {
        let b = button_look(look, k, spec.button);
        b.enabled && !b.active && !b.pressed
    })
}

/// The light of a button under the pointer, its top left at `(x, y)`: a layer of its own over the piece, fading in and
/// out.
pub fn glow(c: &Canvas<'_>, t: &Theme, u: f32, (x, y): (f32, f32)) -> Result<()> {
    c.fill_round(rect(x, y, BUTTON * u, BUTTON * u), 6.0 * u, t.subtle_hover)
}

/// The surface of a [`glow`]: where it goes (pixels of its parent), where the light is drawn in it, and its size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowAt {
    pub place: (f32, f32),
    pub offset: (f32, f32),
    pub size: (u32, u32),
}

/// The surface of a [`glow`] at `(x, y)`, pixels of its parent.
pub fn glow_at(u: f32, (x, y): (f32, f32)) -> GlowAt {
    let (gx, gy) = (x.floor(), y.floor());
    let side = (BUTTON * u).ceil() as u32 + 1;
    GlowAt { place: (gx, gy), offset: (x - gx, y - gy), size: (side, side) }
}

/// Room around a tip for its shadow.
pub const TIP_MARGIN: f32 = 12.0;

/// Size in pixels of a tip's surface, and of the bubble in it.
pub fn tip_size(gfx: &Gfx, u: f32, text: &str) -> Result<((u32, u32), (f32, f32))> {
    let (w, _) = gfx.measure(text, 12.0 * u, Font::Ui)?;
    let bubble = ((w + 16.0 * u).ceil(), (26.0 * u).ceil());
    let m = TIP_MARGIN * u;
    Ok((((bubble.0 + 2.0 * m).ceil() as u32, (bubble.1 + 2.0 * m).ceil() as u32), bubble))
}

pub fn tip(c: &Canvas<'_>, t: &Theme, u: f32, text: &str, bubble: (f32, f32)) -> Result<()> {
    let m = TIP_MARGIN * u;
    let r = rect(m, m, bubble.0, bubble.1);
    c.shadow(r, 6.0 * u, 8.0, 0.0, Rgba::hex(0x000000, 0x40))?;
    c.fill_round(r, 6.0 * u, t.flyout)?;
    c.stroke_round(r, 6.0 * u, 1.0, t.stroke_strong)?;
    c.text(text, r, 12.0 * u, t.text, Align::Centre)
}

/// What the style panel shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelLook {
    pub color: Color,
    pub recent: Vec<Color>,
    /// The thickness setting (the slider, up to `max_width`) and the size of what the tool draws (shown).
    pub width: f32,
    pub max_width: f32,
    pub size: f32,
    pub filled: bool,
    pub tool: Option<Tool>,
    pub form: PanelForm,
    pub hsv: (f32, f32, f32),
    pub eyedropper: bool,
    pub hover: Option<PanelHit>,
}

pub fn panel_surface(u: f32) -> (u32, u32) {
    ((layout::PANEL_W * u + 2.0 * SHADOW * u).ceil() as u32, (layout::PANEL_MAX_H * u + 2.0 * SHADOW * u).ceil() as u32)
}

fn swatch(c: &Canvas<'_>, t: &Theme, u: f32, x: f32, y: f32, fill: Option<Rgba>, chosen: bool) -> Result<()> {
    let r = layout::SWATCH * u / 2.0;
    let (cx, cy) = (x + r, y + r);
    if let Some(fill) = fill {
        c.fill_circle(cx, cy, r, fill)?;
    }
    if chosen {
        c.stroke_circle(cx, cy, r, 2.0 * u, t.accent)
    } else {
        c.stroke_circle(cx, cy, r, 1.0 * u, t.stroke_strong)
    }
}

pub fn panel(c: &Canvas<'_>, t: &Theme, u: f32, look: &PanelLook) -> Result<()> {
    let (ox, oy) = (SHADOW * u, SHADOW * u);
    let form = &look.form;
    let back = rect(ox, oy, layout::PANEL_W * u, form.height() * u);
    floating_box(c, t, back, 12.0 * u, 0.97, Rgba::hex(0x000000, 0x50))?;
    let at = |x: f32, y: f32| (ox + x * u, oy + y * u);
    let divider = |y: f32| c.fill_rect(rect(ox + 10.0 * u, oy + y * u, (layout::PANEL_W - 20.0) * u, 1.0), t.divider);
    for (i, colour) in Color::PALETTE.iter().enumerate() {
        let (x, y) = at(layout::swatch_x(i), 10.0);
        swatch(c, t, u, x, y, Some(rgba(*colour)), *colour == look.color)?;
    }
    for (i, colour) in look.recent.iter().take(layout::RECENT_SHOWN).enumerate() {
        let (x, y) = at(layout::swatch_x(i), 42.0);
        swatch(c, t, u, x, y, Some(rgba(*colour)), *colour == look.color)?;
    }
    // The colour picker ends the first row.
    let (x, y) = at(layout::OWN_COLOUR.0, layout::OWN_COLOUR.1);
    swatch(c, t, u, x, y, None, form.picker)?;
    c.icon(icons::ADD, x + 4.0 * u, y + 4.0 * u, 16.0 * u, t.text)?;
    let button = |hit: PanelHit, active: bool| ButtonLook {
        active,
        enabled: true,
        hover: look.hover == Some(hit),
        pressed: false,
    };
    icon_button(
        c,
        t,
        u,
        at(layout::EYEDROPPER.0, layout::EYEDROPPER.1),
        icons::EYEDROPPER,
        &button(PanelHit::Eyedropper, look.eyedropper),
    )?;
    divider(74.0)?;

    // The size: a slider, filled up to its knob, and the size of what the tool draws.
    let (tx, ty, length) = layout::SIZE_TRACK;
    let (x, y) = at(tx, ty);
    let knob = x + layout::size_to_slider(look.width, look.max_width) * length * u;
    let track = rect(x, y - 2.0 * u, length * u, 4.0 * u);
    c.fill_round(track, 2.0 * u, t.divider)?;
    c.fill_round(rect(x, y - 2.0 * u, knob - x, 4.0 * u), 2.0 * u, t.accent)?;
    let grow = if look.hover == Some(PanelHit::Size) { 1.0 } else { 0.0 };
    c.fill_circle(knob, y, (7.0 + grow) * u, t.flyout)?;
    c.fill_circle(knob, y, (4.0 + grow) * u, t.accent)?;
    c.stroke_circle(knob, y, (7.0 + grow) * u, 1.0, t.stroke_strong)?;
    let (lx, lw) = layout::SIZE_LABEL;
    let (x, y) = at(lx, ty - 15.0);
    c.text(&format!("{} px", look.size.round()), rect(x, y, lw * u, 30.0 * u), 13.0 * u, t.text, Align::Centre)?;

    // The fill of the shapes.
    if form.fill {
        divider(118.0)?;
        {
            let (x, y) = at(layout::FILL.0, layout::FILL.1);
            button_back(c, t, u, x, y, &button(PanelHit::Fill, false))?;
            // A square in the colour: filled, or its outline only.
            let square = rect(x + 7.0 * u, y + 7.0 * u, 16.0 * u, 16.0 * u);
            if look.filled {
                c.fill_round(square, 3.0 * u, rgba(look.color))?;
                c.stroke_round(square, 3.0 * u, 1.0, t.stroke_strong)?;
            } else {
                c.stroke_round(square, 3.0 * u, 2.0 * u, rgba(look.color))?;
            }
        }
    }

    if form.picker {
        divider(form.picker_top())?;
        let (h, s, v) = look.hsv;
        let (sx, sy, sw, sh) = form.shade();
        let (x, y) = at(sx, sy);
        let shade = rect(x, y, sw * u, sh * u);
        let (r, g, b) = hsv_to_rgb(h, 1.0, 1.0);
        c.fill_round(shade, 4.0 * u, Rgba([r, g, b, 1.0]))?;
        let white = Rgba([1.0, 1.0, 1.0, 1.0]);
        let black = Rgba([0.0, 0.0, 0.0, 1.0]);
        c.gradient(
            shade,
            4.0 * u,
            (shade.left, shade.top),
            (shade.right, shade.top),
            &[(0.0, white), (1.0, white.with_alpha(0.0))],
        )?;
        c.gradient(
            shade,
            4.0 * u,
            (shade.left, shade.top),
            (shade.left, shade.bottom),
            &[(0.0, black.with_alpha(0.0)), (1.0, black)],
        )?;
        let (kx, ky) = (x + s * sw * u, y + (1.0 - v) * sh * u);
        c.stroke_circle(kx, ky, 5.0 * u, 2.0 * u, white)?;
        let (hx, hy, hw, hh) = form.hue();
        let (x, y) = at(hx, hy);
        let bar = rect(x, y, hw * u, hh * u);
        let stops: Vec<(f32, Rgba)> = [0xff0000, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff, 0xff0000]
            .iter()
            .enumerate()
            .map(|(i, c)| (i as f32 / 6.0, Rgba::hex(*c, 255)))
            .collect();
        c.gradient(bar, 4.0 * u, (bar.left, bar.top), (bar.left, bar.bottom), &stops)?;
        let my = y + h / 360.0 * hh * u - 2.0 * u;
        let marker = rect(x, my, hw * u, 4.0 * u);
        c.fill_rect(marker, white)?;
        c.stroke_round(marker, 0.0, 1.0, black)?;
    }
    Ok(())
}

/// Side in pixels of the surface of the size ring for a `diameter` (image pixels).
pub fn brush_side(diameter: u32) -> u32 {
    diameter + 6
}

/// The size of what the tool draws, around the pointer: a ring of that diameter, white over a dark edge so it shows on
/// any picture.
pub fn brush(c: &Canvas<'_>, diameter: u32) -> Result<()> {
    let side = brush_side(diameter) as f32;
    let (centre, r) = (side / 2.0, (diameter as f32 / 2.0).max(1.0));
    c.stroke_circle(centre, centre, r + 1.0, 1.0, Rgba([0.0, 0.0, 0.0, 0.6]))?;
    c.stroke_circle(centre, centre, r, 1.5, Rgba([1.0, 1.0, 1.0, 1.0]))
}

/// Size of the size label for `text`.
pub fn label_size(gfx: &Gfx, u: f32, text: &str) -> Result<(u32, u32)> {
    let (w, _) = gfx.measure(text, 12.0 * u, Font::Ui)?;
    Ok(((w + 16.0 * u).ceil() as u32, (24.0 * u).ceil() as u32))
}

pub fn size_label(c: &Canvas<'_>, t: &Theme, u: f32, text: &str, size: (u32, u32)) -> Result<()> {
    let r = rect(0.0, 0.0, size.0 as f32, size.1 as f32);
    c.fill_round(r, 4.0 * u, t.flyout)?;
    c.stroke_round(r, 4.0 * u, 1.0, t.stroke_strong)?;
    c.text(text, r, 12.0 * u, t.text, Align::Centre)
}

/// Side of a handle's surface.
pub fn handle_side(u: f32) -> u32 {
    (10.0 * u).ceil() as u32 + 2
}

/// A handle of the zone: a white dot ringed with the accent.
pub fn handle(c: &Canvas<'_>, t: &Theme, u: f32) -> Result<()> {
    let side = handle_side(u) as f32;
    let (cx, r) = (side / 2.0, 5.0 * u);
    c.fill_circle(cx, cx, r, Rgba([1.0, 1.0, 1.0, 1.0]))?;
    c.stroke_circle(cx, cx, r, 2.0, t.accent)
}

/// What the text field shows.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldLook {
    pub text: String,
    /// Caret position, in UTF-16 units.
    pub caret: usize,
    pub caret_on: bool,
    pub size: f32,
    pub color: Color,
}

/// Size of the field's surface.
pub fn field_size(gfx: &Gfx, f: &FieldLook) -> Result<(u32, u32)> {
    let (w, _) = gfx.measure(&f.text, f.size, Font::Inter)?;
    Ok(((w + 4.0).ceil() as u32, (f.size * 1.25).ceil() as u32 + 1))
}

pub fn field(c: &Canvas<'_>, f: &FieldLook) -> Result<()> {
    let layout = c.gfx.layout(&f.text, f.size, Font::Inter)?;
    let color = rgba(f.color);
    c.text_layout(&layout, 0.0, 0.0, color)?;
    if f.caret_on {
        let x = Gfx::caret_x(&layout, u32::try_from(f.caret).unwrap_or(0));
        c.fill_rect(rect(x.max(0.0), 0.0, (f.size / 16.0).max(1.5), f.size * 1.25), color)?;
    }
    Ok(())
}
