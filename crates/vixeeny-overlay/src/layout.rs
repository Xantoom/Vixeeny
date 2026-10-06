// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the toolbar's buttons and the style panel's controls are, in interface units (one unit
//! is one physical pixel at 100 %; multiply by the interface scale), and what a point hits.

use vixeeny_editor::Tool;

use crate::icons;

/// Toolbar button index → tool. Index 12 moves and resizes the zone itself (the default).
pub const TOOLS: [Option<Tool>; 13] = [
    Some(Tool::Select),
    Some(Tool::Pen),
    Some(Tool::Line),
    Some(Tool::Arrow),
    Some(Tool::Rect),
    Some(Tool::Ellipse),
    Some(Tool::Text),
    Some(Tool::Highlighter),
    Some(Tool::Blur),
    Some(Tool::Pixelate),
    Some(Tool::Marker),
    Some(Tool::Eyedropper),
    None,
];

pub fn tool_index(tool: Option<Tool>) -> usize {
    TOOLS.iter().position(|t| *t == tool).unwrap_or(12)
}

/// What a toolbar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Picks `TOOLS[i]`.
    Tool(usize),
    /// Opens the style panel (colours, thickness, fill).
    Style,
    Undo,
    Redo,
    Copy,
    Save,
    SaveAs,
    Scroll,
    Ocr,
    Close,
}

pub struct ButtonSpec {
    pub button: Button,
    pub x: f32,
    pub icon: &'static str,
}

const fn spec(button: Button, x: f32, icon: &'static str) -> ButtonSpec {
    ButtonSpec { button, x, icon }
}

/// The bar, in its groups: the zone · drawing · shapes and text · masks · style · history ·
/// outputs · close. The order is the order of the tips the host gives.
pub const BUTTONS: [ButtonSpec; 21] = [
    spec(Button::Tool(12), 6.0, icons::MOVE),
    spec(Button::Tool(0), 40.0, icons::CURSOR),
    spec(Button::Tool(1), 87.0, icons::PEN),
    spec(Button::Tool(7), 121.0, icons::HIGHLIGHT),
    spec(Button::Tool(2), 155.0, icons::LINE),
    spec(Button::Tool(3), 189.0, icons::ARROW),
    spec(Button::Tool(4), 236.0, icons::SQUARE),
    spec(Button::Tool(5), 270.0, icons::CIRCLE),
    spec(Button::Tool(6), 304.0, icons::TEXT),
    spec(Button::Tool(10), 338.0, icons::MARKER),
    spec(Button::Tool(8), 385.0, icons::BLUR),
    spec(Button::Tool(9), 419.0, icons::PIXELATE),
    spec(Button::Style, 466.0, ""),
    spec(Button::Undo, 513.0, icons::UNDO),
    spec(Button::Redo, 547.0, icons::REDO),
    spec(Button::Copy, 594.0, icons::COPY),
    spec(Button::Save, 628.0, icons::SAVE),
    spec(Button::SaveAs, 662.0, icons::SAVE_AS),
    spec(Button::Scroll, 696.0, icons::SCROLL),
    spec(Button::Ocr, 730.0, icons::SCAN_TEXT),
    spec(Button::Close, 777.0, icons::DISMISS),
];

/// The hairlines between the groups.
pub const SEPARATORS: [f32; 7] = [78.0, 227.0, 376.0, 457.0, 504.0, 585.0, 768.0];

pub const BAR_W: f32 = 813.0;
pub const BAR_H: f32 = 46.0;
pub const BUTTON: f32 = 30.0;
pub const BUTTON_Y: f32 = 8.0;
/// Room around the bar and the panel for their shadow.
pub const SHADOW: f32 = 28.0;
/// The tip appears after this much hover.
pub const TIP_DELAY_MS: u32 = 450;

/// The button under `(x, y)`, relative to the bar's top-left corner, in units.
pub fn button_at(x: f32, y: f32) -> Option<usize> {
    if !(BUTTON_Y..BUTTON_Y + BUTTON).contains(&y) {
        return None;
    }
    BUTTONS
        .iter()
        .position(|b| (b.x..b.x + BUTTON).contains(&x))
}

// ---- the style panel

pub const PANEL_W: f32 = 240.0;
pub const PANEL_H: f32 = 110.0;
pub const PANEL_PICKER_H: f32 = 250.0;
/// Recent colours shown (the row ends at the "own colour" swatch).
pub const RECENT_SHOWN: usize = 5;
pub const SWATCH: f32 = 24.0;
/// Centre of the style button, where the panel is centred.
pub const STYLE_CENTRE: f32 = 481.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHit {
    Palette(usize),
    Recent(usize),
    /// Opens or closes the colour picker.
    OwnColour,
    Eyedropper,
    Thinner,
    Thicker,
    Fill,
    /// Saturation and value.
    Shade,
    Hue,
    /// Inside the panel, on nothing.
    Background,
}

pub fn swatch_x(i: usize) -> f32 {
    10.0 + i as f32 * 28.0
}

pub const OWN_COLOUR: (f32, f32) = (174.0, 42.0);
pub const EYEDROPPER: (f32, f32) = (202.0, 39.0);
pub const THINNER: (f32, f32) = (6.0, 78.0);
pub const THICKER: (f32, f32) = (72.0, 78.0);
pub const FILL: (f32, f32) = (PANEL_W - 36.0, 78.0);
pub const WIDTH_LABEL: (f32, f32, f32) = (36.0, 78.0, 36.0);
pub const SHADE: (f32, f32, f32, f32) = (10.0, 120.0, 180.0, 120.0);
pub const HUE: (f32, f32, f32, f32) = (202.0, 120.0, 28.0, 120.0);

fn in_box(x: f32, y: f32, at: (f32, f32), w: f32, h: f32) -> bool {
    (at.0..at.0 + w).contains(&x) && (at.1..at.1 + h).contains(&y)
}

/// What `(x, y)` (relative to the panel's top-left corner, in units) is on.
pub fn panel_at(x: f32, y: f32, palette: usize, recent: usize, picker: bool) -> Option<PanelHit> {
    let height = if picker { PANEL_PICKER_H } else { PANEL_H };
    if !(0.0..PANEL_W).contains(&x) || !(0.0..height).contains(&y) {
        return None;
    }
    let hit = if let Some(i) =
        (0..palette).find(|&i| in_box(x, y, (swatch_x(i), 10.0), SWATCH, SWATCH))
    {
        PanelHit::Palette(i)
    } else if let Some(i) =
        (0..recent.min(RECENT_SHOWN)).find(|&i| in_box(x, y, (swatch_x(i), 42.0), SWATCH, SWATCH))
    {
        PanelHit::Recent(i)
    } else if in_box(x, y, OWN_COLOUR, SWATCH, SWATCH) {
        PanelHit::OwnColour
    } else if in_box(x, y, EYEDROPPER, BUTTON, BUTTON) {
        PanelHit::Eyedropper
    } else if in_box(x, y, THINNER, BUTTON, BUTTON) {
        PanelHit::Thinner
    } else if in_box(x, y, THICKER, BUTTON, BUTTON) {
        PanelHit::Thicker
    } else if in_box(x, y, FILL, BUTTON, BUTTON) {
        PanelHit::Fill
    } else if picker && in_box(x, y, (SHADE.0, SHADE.1), SHADE.2, SHADE.3) {
        PanelHit::Shade
    } else if picker && in_box(x, y, (HUE.0, HUE.1), HUE.2, HUE.3) {
        PanelHit::Hue
    } else {
        PanelHit::Background
    };
    Some(hit)
}

/// Saturation and value at `(x, y)` (panel units), clamped to the shade square.
pub fn shade_at(x: f32, y: f32) -> (f32, f32) {
    let s = ((x - SHADE.0) / SHADE.2).clamp(0.0, 1.0);
    let v = 1.0 - ((y - SHADE.1) / SHADE.3).clamp(0.0, 1.0);
    (s, v)
}

/// Hue (degrees) at panel height `y`.
pub fn hue_at(y: f32) -> f32 {
    360.0 * ((y - HUE.1) / HUE.3).clamp(0.0, 0.999)
}

/// Where the panel goes, relative to the bar (units): centred on the style button, above the bar
/// when there is room (`room_above`, units), else below.
pub fn panel_offset(room_above: f32, picker: bool) -> (f32, f32) {
    let height = if picker { PANEL_PICKER_H } else { PANEL_H };
    let x = STYLE_CENTRE - PANEL_W / 2.0;
    if room_above > height + 8.0 {
        (x, -height - 8.0)
    } else {
        (x, BAR_H + 8.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_are_found_by_position() {
        assert_eq!(button_at(10.0, 20.0), Some(0));
        assert_eq!(button_at(500.0, 20.0), None); // between undo and the style button
        assert_eq!(button_at(470.0, 20.0), Some(12));
        assert_eq!(button_at(800.0, 20.0), Some(20));
        assert_eq!(button_at(10.0, 2.0), None);
        assert!(matches!(BUTTONS[20].button, Button::Close));
    }

    #[test]
    fn buttons_do_not_overlap_the_separators() {
        for s in SEPARATORS {
            assert!(button_at(s, 20.0).is_none(), "separator at {s}");
        }
    }

    #[test]
    fn the_panel_finds_its_controls() {
        assert_eq!(
            panel_at(12.0, 12.0, 8, 0, false),
            Some(PanelHit::Palette(0))
        );
        assert_eq!(
            panel_at(12.0, 44.0, 8, 0, false),
            Some(PanelHit::Background)
        );
        assert_eq!(panel_at(12.0, 44.0, 8, 2, false), Some(PanelHit::Recent(0)));
        assert_eq!(
            panel_at(180.0, 50.0, 8, 0, false),
            Some(PanelHit::OwnColour)
        );
        assert_eq!(panel_at(20.0, 150.0, 8, 0, false), None);
        assert_eq!(panel_at(20.0, 150.0, 8, 0, true), Some(PanelHit::Shade));
        assert_eq!(panel_at(210.0, 150.0, 8, 0, true), Some(PanelHit::Hue));
    }

    #[test]
    fn the_panel_opens_below_near_the_top() {
        assert_eq!(panel_offset(500.0, false).1, -PANEL_H - 8.0);
        assert_eq!(panel_offset(20.0, false).1, BAR_H + 8.0);
    }

    #[test]
    fn shade_and_hue_are_clamped() {
        assert_eq!(shade_at(0.0, 0.0), (0.0, 1.0));
        assert_eq!(shade_at(1000.0, 1000.0), (1.0, 0.0));
        assert!(hue_at(1000.0) < 360.0);
    }
}
