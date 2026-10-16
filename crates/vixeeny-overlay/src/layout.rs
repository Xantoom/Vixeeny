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
    /// Opens the style panel (colour, size, the button's other tool, fill).
    Style,
    Undo,
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

/// The bar, in its groups: the zone · the tools · style · undo · outputs · close.
pub const BUTTONS: [ButtonSpec; 13] = [
    spec(Button::Tool(Kit::Zone), 6.0),
    spec(Button::Tool(Kit::Pen), 53.0),
    spec(Button::Tool(Kit::Highlighter), 87.0),
    spec(Button::Tool(Kit::Arrow), 121.0),
    spec(Button::Tool(Kit::Shape), 155.0),
    spec(Button::Tool(Kit::Text), 189.0),
    spec(Button::Tool(Kit::Marker), 223.0),
    spec(Button::Tool(Kit::Mask), 257.0),
    spec(Button::Style, 304.0),
    spec(Button::Undo, 351.0),
    spec(Button::Copy, 398.0),
    spec(Button::CopyText, 432.0),
    spec(Button::Save, 466.0),
];

/// The close button, last.
pub const CLOSE: ButtonSpec = spec(Button::Close, 513.0);

/// Every button, close included.
pub fn buttons() -> impl Iterator<Item = &'static ButtonSpec> {
    BUTTONS.iter().chain(std::iter::once(&CLOSE))
}

/// The button `k` of [`buttons`].
pub fn button(k: usize) -> &'static ButtonSpec {
    BUTTONS.get(k).unwrap_or(&CLOSE)
}

/// The hairlines between the groups.
pub const SEPARATORS: [f32; 5] = [44.0, 295.0, 342.0, 389.0, 504.0];

/// Length and thickness of the bar.
pub const BAR_W: f32 = 549.0;
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
    Copy,
    CopyText,
    Save,
    Close,
}

pub const TIP_COUNT: usize = 17;

/// The tip of button `k`: a two-tool button names the tool it takes now.
pub fn tip_of(k: usize, picks: &Picks) -> Tip {
    match button(k).button {
        Button::Tool(kit) => match picks.tool(kit) {
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
        },
        Button::Style => Tip::Style,
        Button::Undo => Tip::Undo,
        Button::Copy => Tip::Copy,
        Button::CopyText => Tip::CopyText,
        Button::Save => Tip::Save,
        Button::Close => Tip::Close,
    }
}

// ---- the style panel

pub const PANEL_W: f32 = 240.0;
/// Recent colours shown (the row ends at the "own colour" swatch).
pub const RECENT_SHOWN: usize = 5;
pub const SWATCH: f32 = 24.0;

/// What the panel holds besides the colours and the size: the other tool of the button in use
/// and the fill of the shapes, and whether the colour picker is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelForm {
    /// The two tools of the button in use.
    pub variants: Option<[Tool; 2]>,
    /// The fill switch (shapes).
    pub fill: bool,
    pub picker: bool,
}

impl PanelForm {
    /// The panel for the tool in hand.
    pub fn of(tool: Option<Tool>, picker: bool) -> Self {
        let tools = Kit::of(tool).map_or(&[][..], Kit::tools);
        Self {
            variants: (tools.len() == 2).then(|| [tools[0], tools[1]]),
            fill: matches!(tool, Some(Tool::Rect | Tool::Ellipse)),
            picker,
        }
    }

    /// The row of the variants and the fill is there.
    pub fn extras(&self) -> bool {
        self.variants.is_some() || self.fill
    }

    /// Where the colour picker starts.
    pub fn picker_top(&self) -> f32 {
        if self.extras() { 166.0 } else { 120.0 }
    }

    pub fn height(&self) -> f32 {
        self.picker_top() + if self.picker { 140.0 } else { 0.0 }
    }

    /// The saturation-and-value square and the hue bar: x, y, w, h.
    pub fn shade(&self) -> (f32, f32, f32, f32) {
        (10.0, self.picker_top() + 10.0, 180.0, 120.0)
    }

    pub fn hue(&self) -> (f32, f32, f32, f32) {
        (202.0, self.picker_top() + 10.0, 28.0, 120.0)
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
    /// One of the two tools of the button in use.
    Variant(usize),
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
/// The slider's track (x, y of its middle, length) and the size shown after it (x, width).
pub const SIZE_TRACK: (f32, f32, f32) = (18.0, 97.0, 164.0);
pub const SIZE_LABEL: (f32, f32) = (190.0, 44.0);
/// The row of the variants and the fill.
pub const EXTRAS_Y: f32 = 126.0;
pub fn variant_x(i: usize) -> f32 {
    10.0 + i as f32 * 34.0
}
pub const FILL_X: f32 = PANEL_W - 40.0;

/// Smallest and largest size the slider gives.
pub const SIZE_MIN: f32 = 1.0;
pub const SIZE_MAX: f32 = 40.0;

/// Where the slider's knob is for `width` (0–1): the small sizes, the most used, get more room.
pub fn size_to_slider(width: f32) -> f32 {
    ((width.clamp(SIZE_MIN, SIZE_MAX) - SIZE_MIN) / (SIZE_MAX - SIZE_MIN)).sqrt()
}

/// The width for the pointer at panel `x` on the slider.
pub fn size_at(x: f32) -> f32 {
    let (x0, _, length) = SIZE_TRACK;
    let t = ((x - x0) / length).clamp(0.0, 1.0);
    (SIZE_MIN + (SIZE_MAX - SIZE_MIN) * t * t).round()
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
    let variants = form.variants.map_or(0, |v| v.len());
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
    } else if let Some(i) =
        (0..variants).find(|&i| in_box(x, y, (variant_x(i), EXTRAS_Y), BUTTON, BUTTON))
    {
        PanelHit::Variant(i)
    } else if form.fill && in_box(x, y, (FILL_X, EXTRAS_Y), BUTTON, BUTTON) {
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
        assert_eq!(button_at(40.0, 20.0, false), None); // before the separator
        assert_eq!(button_at(310.0, 20.0, false), Some(8));
        assert_eq!(button_at(20.0, 310.0, true), Some(8));
        assert_eq!(button_at(530.0, 20.0, false), Some(13));
        assert_eq!(button_at(10.0, 2.0, false), None);
        assert!(matches!(button(13).button, Button::Close));
        assert_eq!(button_xy(8, true), (BUTTON_Y, 304.0));
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
    fn two_tool_buttons_take_what_the_panel_chose() {
        let mut picks = Picks::default();
        assert_eq!(picks.tool(Kit::Shape), Some(Tool::Rect));
        picks.remember(Tool::Ellipse);
        picks.remember(Tool::Pen);
        assert_eq!(picks.tool(Kit::Shape), Some(Tool::Ellipse));
        assert_eq!(picks.tool(Kit::Arrow), Some(Tool::Arrow));
        assert_eq!(Kit::of(Some(Tool::Pixelate)), Some(Kit::Mask));
        assert_eq!(Kit::of(None), Some(Kit::Zone));
        assert_eq!(Kit::of(Some(Tool::Eyedropper)), None);
        let shape = BUTTONS
            .iter()
            .position(|b| b.button == Button::Tool(Kit::Shape))
            .unwrap_or(0);
        assert_eq!(tip_of(shape, &picks), Tip::Ellipse);
        assert_eq!(tip_of(13, &picks), Tip::Close);
    }

    #[test]
    fn the_panel_fits_the_tool() {
        let pen = PanelForm::of(Some(Tool::Pen), false);
        assert!(!pen.extras());
        let rect = PanelForm::of(Some(Tool::Rect), false);
        assert_eq!(rect.variants, Some([Tool::Rect, Tool::Ellipse]));
        assert!(rect.fill && rect.height() > pen.height());
        let blur = PanelForm::of(Some(Tool::Pixelate), true);
        assert!(!blur.fill && blur.variants.is_some());
        assert_eq!(blur.height(), PANEL_MAX_H);
    }

    #[test]
    fn the_panel_finds_its_controls() {
        let pen = PanelForm::of(Some(Tool::Pen), false);
        let rect = PanelForm::of(Some(Tool::Rect), true);
        assert_eq!(panel_at(12.0, 12.0, 8, 0, &pen), Some(PanelHit::Palette(0)));
        assert_eq!(panel_at(12.0, 44.0, 8, 0, &pen), Some(PanelHit::Background));
        assert_eq!(panel_at(12.0, 44.0, 8, 2, &pen), Some(PanelHit::Recent(0)));
        assert_eq!(panel_at(180.0, 50.0, 8, 0, &pen), Some(PanelHit::OwnColour));
        assert_eq!(panel_at(100.0, 97.0, 8, 0, &pen), Some(PanelHit::Size));
        assert_eq!(panel_at(20.0, 140.0, 8, 0, &pen), None);
        assert_eq!(
            panel_at(20.0, 140.0, 8, 0, &rect),
            Some(PanelHit::Variant(0))
        );
        assert_eq!(
            panel_at(50.0, 140.0, 8, 0, &rect),
            Some(PanelHit::Variant(1))
        );
        assert_eq!(panel_at(210.0, 140.0, 8, 0, &rect), Some(PanelHit::Fill));
        assert_eq!(panel_at(20.0, 200.0, 8, 0, &rect), Some(PanelHit::Shade));
        assert_eq!(panel_at(210.0, 200.0, 8, 0, &rect), Some(PanelHit::Hue));
    }

    #[test]
    fn the_slider_gives_small_sizes_more_room() {
        let (x0, _, length) = SIZE_TRACK;
        assert_eq!(size_at(x0 - 50.0), SIZE_MIN);
        assert_eq!(size_at(x0 + length + 50.0), SIZE_MAX);
        assert!(size_at(x0 + length / 2.0) < 15.0);
        for w in [1.0, 4.0, 12.0, 40.0] {
            assert_eq!(size_at(x0 + size_to_slider(w) * length), w);
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
