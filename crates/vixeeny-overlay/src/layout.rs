// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the toolbar's buttons and the style panel's controls are, in interface units (one unit
//! is one physical pixel at 100 %; multiply by the interface scale), and what a point hits.
//!
//! The bar lies above or below the zone and stands upright beside it: its buttons are laid
//! along its length (`along`) the same way in both, only the axes swap.

use vixeeny_editor::Tool;

use crate::icons;

/// A tool button of the bar. Some hold two tools: the panel chooses which one the button takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kit {
    /// Moves and resizes the zone, and the annotations pressed on (no tool in hand).
    Zone,
    Pen,
    Highlighter,
    /// The arrow, or the plain line.
    Arrow,
    /// The rectangle, or the ellipse.
    Shape,
    Text,
    Marker,
    /// The blur, or the mosaic.
    Mask,
}

impl Kit {
    /// The tools the button can take; the first is its default.
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Self::Zone => &[],
            Self::Pen => &[Tool::Pen],
            Self::Highlighter => &[Tool::Highlighter],
            Self::Arrow => &[Tool::Arrow, Tool::Line],
            Self::Shape => &[Tool::Rect, Tool::Ellipse],
            Self::Text => &[Tool::Text],
            Self::Marker => &[Tool::Marker],
            Self::Mask => &[Tool::Blur, Tool::Pixelate],
        }
    }

    /// The button that holds `tool` (`None`: the zone).
    pub fn of(tool: Option<Tool>) -> Option<Self> {
        let Some(tool) = tool else {
            return Some(Self::Zone);
        };
        KITS.into_iter().find(|k| k.tools().contains(&tool))
    }
}

const KITS: [Kit; 8] = [
    Kit::Zone,
    Kit::Pen,
    Kit::Highlighter,
    Kit::Arrow,
    Kit::Shape,
    Kit::Text,
    Kit::Marker,
    Kit::Mask,
];

/// The tool each two-tool button takes now: what the panel last chose for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picks {
    pub arrow: Tool,
    pub shape: Tool,
    pub mask: Tool,
}

impl Default for Picks {
    fn default() -> Self {
        Self {
            arrow: Tool::Arrow,
            shape: Tool::Rect,
            mask: Tool::Blur,
        }
    }
}

impl Picks {
    /// The tool `kit`'s button takes (`None`: the zone).
    pub fn tool(&self, kit: Kit) -> Option<Tool> {
        match kit {
            Kit::Zone => None,
            Kit::Arrow => Some(self.arrow),
            Kit::Shape => Some(self.shape),
            Kit::Mask => Some(self.mask),
            other => other.tools().first().copied(),
        }
    }

    /// Remembers `tool` as its button's choice.
    pub fn remember(&mut self, tool: Tool) {
        match Kit::of(Some(tool)) {
            Some(Kit::Arrow) => self.arrow = tool,
            Some(Kit::Shape) => self.shape = tool,
            Some(Kit::Mask) => self.mask = tool,
            _ => {}
        }
    }
}

/// The icon of a tool.
pub fn tool_icon(tool: Option<Tool>) -> &'static str {
    match tool {
        None | Some(Tool::Select) => icons::MOVE,
        Some(Tool::Pen) => icons::PEN,
        Some(Tool::Highlighter) => icons::HIGHLIGHT,
        Some(Tool::Arrow) => icons::ARROW,
        Some(Tool::Line) => icons::LINE,
        Some(Tool::Rect) => icons::SQUARE,
        Some(Tool::Ellipse) => icons::CIRCLE,
        Some(Tool::Text) => icons::TEXT,
        Some(Tool::Marker) => icons::MARKER,
        Some(Tool::Blur) => icons::BLUR,
        Some(Tool::Pixelate) => icons::PIXELATE,
        Some(Tool::Eyedropper) => icons::EYEDROPPER,
    }
}

/// What a toolbar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Tool(Kit),
    /// Shows the screens live behind the zone, or frozen again.
    Live,
    /// Opens the style panel (colours, size, fill).
    Style,
    Undo,
    Redo,
    /// Removes every annotation.
    Clear,
    Copy,
    CopyText,
    Save,
    Close,
}

pub struct ButtonSpec {
    pub button: Button,
    /// Where it starts along the bar.
    pub along: f32,
}

const fn spec(button: Button, along: f32) -> ButtonSpec {
    ButtonSpec { button, along }
}

/// The bar, in its groups: the zone · the tools · style · history · outputs · close.
pub const BUTTONS: [ButtonSpec; 16] = [
    spec(Button::Tool(Kit::Zone), 6.0),
    spec(Button::Live, 40.0),
    spec(Button::Tool(Kit::Pen), 87.0),
    spec(Button::Tool(Kit::Highlighter), 121.0),
    spec(Button::Tool(Kit::Arrow), 155.0),
    spec(Button::Tool(Kit::Shape), 189.0),
    spec(Button::Tool(Kit::Text), 223.0),
    spec(Button::Tool(Kit::Marker), 257.0),
    spec(Button::Tool(Kit::Mask), 291.0),
    spec(Button::Style, 338.0),
    spec(Button::Undo, 385.0),
    spec(Button::Redo, 419.0),
    spec(Button::Clear, 453.0),
    spec(Button::Copy, 500.0),
    spec(Button::CopyText, 534.0),
    spec(Button::Save, 568.0),
];

/// The close button, last.
pub const CLOSE: ButtonSpec = spec(Button::Close, 615.0);

/// Every button, close included.
pub fn buttons() -> impl Iterator<Item = &'static ButtonSpec> {
    BUTTONS.iter().chain(std::iter::once(&CLOSE))
}

/// The button `k` of [`buttons`].
pub fn button(k: usize) -> &'static ButtonSpec {
    BUTTONS.get(k).unwrap_or(&CLOSE)
}

/// The hairlines between the groups.
pub const SEPARATORS: [f32; 5] = [78.0, 329.0, 376.0, 491.0, 606.0];

/// Length and thickness of the bar.
pub const BAR_W: f32 = 651.0;
pub const BAR_H: f32 = 46.0;
pub const BUTTON: f32 = 30.0;
pub const BUTTON_Y: f32 = 8.0;
/// Room around the bar and the panel for their shadow.
pub const SHADOW: f32 = 28.0;
/// The tip appears after this much hover.
pub const TIP_DELAY_MS: u32 = 450;

/// The bar's size, lying or upright.
pub fn bar_size(upright: bool) -> (f32, f32) {
    if upright {
        (BAR_H, BAR_W)
    } else {
        (BAR_W, BAR_H)
    }
}

/// Where button `k` is, relative to the bar's top-left corner.
pub fn button_xy(k: usize, upright: bool) -> (f32, f32) {
    let along = button(k).along;
    if upright {
        (BUTTON_Y, along)
    } else {
        (along, BUTTON_Y)
    }
}

/// The button under `(x, y)`, relative to the bar's top-left corner, in units.
pub fn button_at(x: f32, y: f32, upright: bool) -> Option<usize> {
    let (along, across) = if upright { (y, x) } else { (x, y) };
    if !(BUTTON_Y..BUTTON_Y + BUTTON).contains(&across) {
        return None;
    }
    buttons().position(|b| (b.along..b.along + BUTTON).contains(&along))
}

/// The tips, in this order; the host gives their texts (see [`tip_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tip {
    Zone,
    /// The live button, while the screens are frozen, and while they are live.
    Live,
    Freeze,
    Pen,
    Highlighter,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Marker,
    Blur,
    Pixelate,
    Style,
    Undo,
    Redo,
    Clear,
    Copy,
    CopyText,
    Save,
    Close,
    /// What comes before the other tool of a two-tool button ("Right-click: ").
    RightClick,
}

pub const TIP_COUNT: usize = 22;

fn tool_tip(tool: Option<Tool>) -> Tip {
    match tool {
        None | Some(Tool::Select | Tool::Eyedropper) => Tip::Zone,
        Some(Tool::Pen) => Tip::Pen,
        Some(Tool::Highlighter) => Tip::Highlighter,
        Some(Tool::Arrow) => Tip::Arrow,
        Some(Tool::Line) => Tip::Line,
        Some(Tool::Rect) => Tip::Rect,
        Some(Tool::Ellipse) => Tip::Ellipse,
        Some(Tool::Text) => Tip::Text,
        Some(Tool::Marker) => Tip::Marker,
        Some(Tool::Blur) => Tip::Blur,
        Some(Tool::Pixelate) => Tip::Pixelate,
    }
}

/// The tip of button `k` (`live`: the screens are shown live), and for a two-tool button the
/// tip of its other tool, which a right-click takes.
pub fn tip_of(k: usize, picks: &Picks, live: bool) -> (Tip, Option<Tip>) {
    let tip = match button(k).button {
        Button::Tool(kit) => {
            return (
                tool_tip(picks.tool(kit)),
                other_tool(kit, picks).map(|t| tool_tip(Some(t))),
            );
        }
        Button::Live if live => Tip::Freeze,
        Button::Live => Tip::Live,
        Button::Style => Tip::Style,
        Button::Undo => Tip::Undo,
        Button::Redo => Tip::Redo,
        Button::Clear => Tip::Clear,
        Button::Copy => Tip::Copy,
        Button::CopyText => Tip::CopyText,
        Button::Save => Tip::Save,
        Button::Close => Tip::Close,
    };
    (tip, None)
}

/// The tool a right-click on `kit`'s button takes: the one it does not take now.
pub fn other_tool(kit: Kit, picks: &Picks) -> Option<Tool> {
    let now = picks.tool(kit)?;
    kit.tools().iter().copied().find(|t| *t != now)
}

// ---- the style panel
//
// Three rows: the colours given, ended by the colour picker; the colours last used, ended by
// the eyedropper; the size, ended by its value. The shapes add their fill; the picker opens
// under it all.

pub const PANEL_W: f32 = 268.0;
/// Recent colours shown.
pub const RECENT_SHOWN: usize = 8;
pub const SWATCH: f32 = 24.0;

/// What the panel holds besides the colours and the size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelForm {
    /// The fill switch (shapes).
    pub fill: bool,
    /// The colour picker is open.
    pub picker: bool,
}

impl PanelForm {
    /// The panel for the tool in hand.
    pub fn of(tool: Option<Tool>, picker: bool) -> Self {
        Self {
            fill: matches!(tool, Some(Tool::Rect | Tool::Ellipse)),
            picker,
        }
    }

    /// Where the colour picker starts.
    pub fn picker_top(&self) -> f32 {
        if self.fill { 166.0 } else { 120.0 }
    }

    pub fn height(&self) -> f32 {
        self.picker_top() + if self.picker { 140.0 } else { 0.0 }
    }

    /// The saturation-and-value square and the hue bar: x, y, w, h.
    pub fn shade(&self) -> (f32, f32, f32, f32) {
        (10.0, self.picker_top() + 10.0, 210.0, 120.0)
    }

    pub fn hue(&self) -> (f32, f32, f32, f32) {
        (230.0, self.picker_top() + 10.0, 28.0, 120.0)
    }
}

/// The tallest panel (its surface is made once that size).
pub const PANEL_MAX_H: f32 = 166.0 + 140.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHit {
    Palette(usize),
    Recent(usize),
    /// Opens or closes the colour picker.
    OwnColour,
    Eyedropper,
    /// The size slider.
    Size,
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

/// The picker's swatch ends the first row, the eyedropper the second.
pub const OWN_COLOUR: (f32, f32) = (234.0, 10.0);
pub const EYEDROPPER: (f32, f32) = (231.0, 39.0);
/// The slider's track (x, y of its middle, length) and its value after it (x, width).
pub const SIZE_TRACK: (f32, f32, f32) = (18.0, 97.0, 180.0);
pub const SIZE_LABEL: (f32, f32) = (206.0, 54.0);
/// The fill switch of the shapes.
pub const FILL: (f32, f32) = (10.0, 126.0);

/// Where the slider's knob is for `width` (0–1) when sizes go up to `max`: the small sizes,
/// the most used, get more room.
pub fn size_to_slider(width: f32, max: f32) -> f32 {
    ((width.clamp(1.0, max) - 1.0) / (max - 1.0).max(1.0)).sqrt()
}

/// The width for the pointer at panel `x` on the slider, sizes going up to `max`.
pub fn size_at(x: f32, max: f32) -> f32 {
    let (x0, _, length) = SIZE_TRACK;
    let t = ((x - x0) / length).clamp(0.0, 1.0);
    (1.0 + (max - 1.0) * t * t).round()
}

fn in_box(x: f32, y: f32, at: (f32, f32), w: f32, h: f32) -> bool {
    (at.0..at.0 + w).contains(&x) && (at.1..at.1 + h).contains(&y)
}

/// What `(x, y)` (relative to the panel's top-left corner, in units) is on.
pub fn panel_at(
    x: f32,
    y: f32,
    palette: usize,
    recent: usize,
    form: &PanelForm,
) -> Option<PanelHit> {
    if !(0.0..PANEL_W).contains(&x) || !(0.0..form.height()).contains(&y) {
        return None;
    }
    let (sx, sy, sl) = SIZE_TRACK;
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
    } else if in_box(x, y, (sx - 10.0, sy - 14.0), sl + 20.0, 28.0) {
        PanelHit::Size
    } else if form.fill && in_box(x, y, FILL, BUTTON, BUTTON) {
        PanelHit::Fill
    } else if form.picker && {
        let (sx, sy, sw, sh) = form.shade();
        in_box(x, y, (sx, sy), sw, sh)
    } {
        PanelHit::Shade
    } else if form.picker && {
        let (hx, hy, hw, hh) = form.hue();
        in_box(x, y, (hx, hy), hw, hh)
    } {
        PanelHit::Hue
    } else {
        PanelHit::Background
    };
    Some(hit)
}

/// Saturation and value at `(x, y)` (panel units), clamped to the shade square.
pub fn shade_at(form: &PanelForm, x: f32, y: f32) -> (f32, f32) {
    let (sx, sy, sw, sh) = form.shade();
    let s = ((x - sx) / sw).clamp(0.0, 1.0);
    let v = 1.0 - ((y - sy) / sh).clamp(0.0, 1.0);
    (s, v)
}

/// Hue (degrees) at panel height `y`.
pub fn hue_at(form: &PanelForm, y: f32) -> f32 {
    let (_, hy, _, hh) = form.hue();
    360.0 * ((y - hy) / hh).clamp(0.0, 0.999)
}

/// The centre of the style button along the bar.
pub fn style_centre() -> f32 {
    buttons()
        .find(|b| b.button == Button::Style)
        .map_or(0.0, |b| b.along + BUTTON / 2.0)
}

/// Where the panel goes, relative to the bar (units), by the style button: before the bar
/// (above it, or left of an upright one) when `before_first` and there is room (`room` before
/// and after the bar, across it), else after it; the other way round when not `before_first`.
pub fn panel_offset(
    upright: bool,
    before_first: bool,
    room: (f32, f32),
    height: f32,
) -> (f32, f32) {
    let (w, h) = (PANEL_W, height);
    // Across the bar: before it (above / left) or after it (below / right).
    let (size, thickness) = if upright { (w, BAR_H) } else { (h, BAR_H) };
    let before_fits = room.0 >= size + 8.0;
    let after_fits = room.1 >= size + 8.0;
    let before = if before_first {
        before_fits || !after_fits
    } else {
        !after_fits && before_fits
    };
    let across = if before { -size - 8.0 } else { thickness + 8.0 };
    let along = style_centre() - if upright { h } else { w } / 2.0;
    if upright {
        (across, along)
    } else {
        (along, across)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_are_found_by_position_lying_and_upright() {
        assert_eq!(button_at(10.0, 20.0, false), Some(0));
        assert_eq!(button_at(20.0, 10.0, true), Some(0));
        assert_eq!(button_at(75.0, 20.0, false), None); // before the separator
        assert_eq!(button_at(345.0, 20.0, false), Some(9));
        assert_eq!(button_at(20.0, 345.0, true), Some(9));
        assert_eq!(button_at(630.0, 20.0, false), Some(16));
        assert_eq!(button_at(10.0, 2.0, false), None);
        assert!(matches!(button(9).button, Button::Style));
        assert!(matches!(button(16).button, Button::Close));
        assert_eq!(button_xy(9, true), (BUTTON_Y, 338.0));
    }

    #[test]
    fn buttons_do_not_overlap_the_separators_nor_each_other() {
        for s in SEPARATORS {
            assert!(button_at(s, 20.0, false).is_none(), "separator at {s}");
        }
        let starts: Vec<f32> = buttons().map(|b| b.along).collect();
        assert!(starts.windows(2).all(|w| w[1] >= w[0] + BUTTON));
        const { assert!(CLOSE.along + BUTTON + 6.0 <= BAR_W) };
    }

    #[test]
    fn the_editor_places_a_bar_of_this_size() {
        assert_eq!(vixeeny_editor::session::TOOLBAR_SIZE, (BAR_W, BAR_H));
    }

    #[test]
    fn a_right_click_takes_the_other_tool_and_the_tip_says_so() {
        let mut picks = Picks::default();
        assert_eq!(other_tool(Kit::Mask, &picks), Some(Tool::Pixelate));
        assert_eq!(other_tool(Kit::Pen, &picks), None);
        picks.remember(Tool::Pixelate);
        picks.remember(Tool::Pen);
        assert_eq!(picks.tool(Kit::Mask), Some(Tool::Pixelate));
        assert_eq!(other_tool(Kit::Mask, &picks), Some(Tool::Blur));
        assert_eq!(Kit::of(Some(Tool::Ellipse)), Some(Kit::Shape));
        assert_eq!(Kit::of(None), Some(Kit::Zone));
        assert_eq!(Kit::of(Some(Tool::Eyedropper)), None);
        let mask = BUTTONS
            .iter()
            .position(|b| b.button == Button::Tool(Kit::Mask))
            .unwrap_or(0);
        assert_eq!(
            tip_of(mask, &picks, false),
            (Tip::Pixelate, Some(Tip::Blur))
        );
        assert_eq!(tip_of(2, &picks, false), (Tip::Pen, None));
        assert_eq!(tip_of(1, &picks, false).0, Tip::Live);
        assert_eq!(tip_of(1, &picks, true).0, Tip::Freeze);
        assert_eq!(tip_of(16, &picks, false).0, Tip::Close);
        assert_eq!(Tip::RightClick as usize, TIP_COUNT - 1);
    }

    #[test]
    fn the_panel_fits_the_tool() {
        let pen = PanelForm::of(Some(Tool::Pen), false);
        let rect = PanelForm::of(Some(Tool::Rect), false);
        assert!(!pen.fill && rect.fill && rect.height() > pen.height());
        assert_eq!(
            PanelForm::of(Some(Tool::Ellipse), true).height(),
            PANEL_MAX_H
        );
    }

    #[test]
    fn the_panel_finds_its_controls() {
        let pen = PanelForm::of(Some(Tool::Pen), false);
        let rect = PanelForm::of(Some(Tool::Rect), true);
        assert_eq!(panel_at(12.0, 12.0, 8, 0, &pen), Some(PanelHit::Palette(0)));
        assert_eq!(panel_at(240.0, 20.0, 8, 0, &pen), Some(PanelHit::OwnColour));
        assert_eq!(panel_at(12.0, 44.0, 8, 0, &pen), Some(PanelHit::Background));
        assert_eq!(panel_at(12.0, 44.0, 8, 2, &pen), Some(PanelHit::Recent(0)));
        assert_eq!(panel_at(210.0, 50.0, 8, 8, &pen), Some(PanelHit::Recent(7)));
        assert_eq!(
            panel_at(245.0, 50.0, 8, 8, &pen),
            Some(PanelHit::Eyedropper)
        );
        assert_eq!(panel_at(100.0, 97.0, 8, 0, &pen), Some(PanelHit::Size));
        assert_eq!(panel_at(20.0, 140.0, 8, 0, &pen), None);
        assert_eq!(panel_at(20.0, 140.0, 8, 0, &rect), Some(PanelHit::Fill));
        assert_eq!(panel_at(20.0, 200.0, 8, 0, &rect), Some(PanelHit::Shade));
        assert_eq!(panel_at(240.0, 200.0, 8, 0, &rect), Some(PanelHit::Hue));
    }

    #[test]
    fn the_slider_gives_small_sizes_more_room() {
        let (x0, _, length) = SIZE_TRACK;
        for max in [40.0, 80.0] {
            assert_eq!(size_at(x0 - 50.0, max), 1.0);
            assert_eq!(size_at(x0 + length + 50.0, max), max);
            assert!(size_at(x0 + length / 2.0, max) < max / 2.0);
            for w in [1.0, 4.0, 12.0, max] {
                assert_eq!(size_at(x0 + size_to_slider(w, max) * length, max), w);
            }
        }
    }

    #[test]
    fn the_panel_opens_away_from_the_zone_when_it_can() {
        let h = 120.0;
        // Above the zone: above the bar.
        assert_eq!(panel_offset(false, true, (500.0, 500.0), h).1, -h - 8.0);
        // No room above: below.
        assert_eq!(panel_offset(false, true, (20.0, 500.0), h).1, BAR_H + 8.0);
        // Below the zone: below the bar.
        assert_eq!(panel_offset(false, false, (500.0, 500.0), h).1, BAR_H + 8.0);
        // Upright on the right: on its right, beside the style button.
        let (x, y) = panel_offset(true, false, (500.0, 500.0), h);
        assert_eq!((x, y), (BAR_H + 8.0, style_centre() - h / 2.0));
        // On the left: on its left.
        assert_eq!(
            panel_offset(true, true, (500.0, 500.0), h).0,
            -PANEL_W - 8.0
        );
    }
}
