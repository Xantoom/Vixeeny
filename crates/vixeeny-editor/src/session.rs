// SPDX-License-Identifier: GPL-3.0-or-later
//! One Print Screen session: the frozen image, the zone, the tools and the commands. Everything the UI needs is read
//! from [`Session::view`]; everything the user does goes through the `pointer_*`, `key` and `choose_*` methods. No UI
//! toolkit is involved.

use std::cell::RefCell;
use std::rc::Rc;

use crate::editor::{Editor, Modifiers, Outcome, Tool};
use crate::geometry::{Point, Rect};
use crate::model::{Annotation, AnnotationId, Color};
use crate::render::{RgbaImage, render_region};
use crate::selection::{BarPlace, CursorHint, Selection, place_label, place_toolbar};

/// The drawing area of the layers grows by blocks of this many pixels (see [`View::layers_area`]).
pub const LAYER_BLOCK: u32 = 128;
/// Drawn around the annotations' bounds: joins and anti-aliasing reach a little past them.
const LAYER_MARGIN: f32 = 8.0;

type LayersKey = (u64, Option<AnnotationId>, Option<Annotation>);
/// Whole image pixels: x, y, width, height.
type Area = (u32, u32, u32, u32);

/// Size of the toolbar, in image pixels (the UI lays out its content to fit).
pub const TOOLBAR_SIZE: (f32, f32) = (651.0, 46.0);
/// The thickness the tools start with, and the largest one, at 100 % (they grow with the interface scale).
pub const DEFAULT_WIDTH: f32 = 4.0;
pub const MAX_WIDTH: f32 = 40.0;
/// Space around the toolbar.
const GAP: f32 = 8.0;
/// Size of the size label at its widest (`9999 × 9999`), in image pixels at 100 %: the UI's is no larger.
pub const LABEL_SIZE: (f32, f32) = (96.0, 24.0);
/// Space between the size label and the zone.
const LABEL_GAP: f32 = 2.0;

/// What the user asked the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Copy the result to the clipboard, then close.
    Copy,
    /// Read the text of the result and copy it, then close.
    CopyText,
    /// Save with the default folder and format.
    Save,
    /// Ask for a folder and a format.
    SaveAs,
    /// Capture the zone while the user scrolls it.
    Scroll,
    /// Close without saving.
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Escape,
    Delete,
    Enter,
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
}

/// Everything to display, recomputed after each input.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub selection: Option<Rect>,
    /// The zone is final for now (not being drawn): handles and toolbar are shown.
    pub settled: bool,
    pub hover_window: Option<Rect>,
    pub size_label: Option<(String, Point)>,
    pub toolbar: Option<BarPlace>,
    /// An annotation is being drawn (the toolbar inside the zone fades away meanwhile).
    pub drawing: bool,
    /// The outline of the selected annotation, while the zone tool is in hand.
    pub selected: Option<Rect>,
    /// The size of what the tool in hand draws (see [`Session::brush`]).
    pub brush: Option<f32>,
    /// Where the pointer is, image pixels.
    pub pointer: Point,
    /// The screens are shown live rather than frozen.
    pub live: bool,
    /// The largest thickness.
    pub max_width: f32,
    /// What to draw over the screens, bottom to top (image pixels): the annotations and the gesture in progress. The
    /// same `Rc` comes back while nothing changes.
    pub layers: Rc<Vec<Annotation>>,
    /// Where they are drawn (whole image pixels: x, y, width, height): what they cover, in the zone, rounded out to
    /// blocks of [`LAYER_BLOCK`] pixels. It only grows while the zone stays, by one more block where it grows, so it
    /// changes seldom while drawing. `None` while there is nothing to draw.
    pub layers_area: Option<(u32, u32, u32, u32)>,
    /// Changes when the picture of the screens is replaced (see [`Session::replace_base`]).
    pub base_version: u64,
    pub cursor: CursorHint,
    pub tool: Option<Tool>,
    pub color: Color,
    pub recent_colors: Vec<Color>,
    pub width: f32,
    pub filled: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// There are annotations (to clear).
    pub can_clear: bool,
    /// Where the text field is open, if the Text tool was just clicked.
    pub text_input: Option<Point>,
    /// Font size of the text tool, in pixels.
    pub text_size: f32,
    pub dim: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Selection,
    Editor,
}

pub struct Session {
    base: RgbaImage,
    editor: Editor,
    selection: Selection,
    tool: Option<Tool>,
    /// The tool to go back to once the eyedropper has picked a colour.
    before_eyedropper: Option<Tool>,
    target: Option<Target>,
    cursor: Point,
    text_input: Option<Point>,
    /// Veil opacity over the image outside the zone (0.0–1.0).
    pub dim: f32,
    /// Command issued once, as soon as the first zone is settled (the scrolling capture: no toolbar step).
    auto_command: Option<Command>,
    /// Scale of the interface (the window's scale factor); the toolbar grows with it.
    ui_scale: f32,
    /// The monitors, in image pixels: the toolbar stays on one of them.
    screens: Vec<Rect>,
    /// The layers last given (see [`View::layers`]), and what they were made from: the document's revision, the
    /// annotation hidden and the gesture in progress.
    layers: RefCell<(Rc<Vec<Annotation>>, Option<LayersKey>)>,
    /// Bounds of the immutable layer list: pointer motion must not rescan every stroke or lay out every text.
    coverage: RefCell<(Rc<Vec<Annotation>>, Option<Rect>)>,
    /// The last layers area given, and the zone it was for (see [`View::layers_area`]).
    area: RefCell<Option<(Area, Area)>>,
    base_version: u64,
    /// The screens are shown live: the annotations are drawn over nothing (see [`Session::set_live`]).
    live: bool,
}

impl Session {
    /// `windows`: rectangles of the visible windows in image pixels, topmost first.
    pub fn new(base: RgbaImage, windows: Vec<Rect>) -> Self {
        let bounds = Rect::new(0.0, 0.0, base.width as f32, base.height as f32);
        Self {
            editor: Editor::new(base.width, base.height),
            selection: Selection::new(bounds, windows),
            base,
            tool: None,
            before_eyedropper: None,
            target: None,
            cursor: Point::default(),
            text_input: None,
            dim: 0.4,
            auto_command: None,
            ui_scale: 1.0,
            screens: vec![bounds],
            layers: RefCell::default(),
            coverage: RefCell::default(),
            area: RefCell::new(None),
            base_version: 0,
            live: false,
        }
    }

    /// The monitors the image spans (image pixels), so that the toolbar never straddles two of them.
    pub fn with_screens(mut self, screens: Vec<Rect>) -> Self {
        if !screens.is_empty() {
            self.screens = screens;
        }
        self
    }

    /// The monitor `p` is on (the nearest one when it is in none).
    fn screen_at(&self, p: Point) -> Rect {
        let distance = |r: &Rect| {
            let dx = (r.x - p.x).max(p.x - r.right()).max(0.0);
            let dy = (r.y - p.y).max(p.y - r.bottom()).max(0.0);
            dx * dx + dy * dy
        };
        self.screens
            .iter()
            .copied()
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
            .unwrap_or_else(|| Rect::new(0.0, 0.0, self.base.width as f32, self.base.height as f32))
    }

    /// A picture shown inside the image (`area`, image pixels) rather than the whole of it: the zone starts as the
    /// picture and never leaves it.
    pub fn within(mut self, area: Rect) -> Self {
        self.selection.limit_to(area);
        self.selection.select_all();
        self
    }

    /// Issues `command` as soon as the first zone is validated (the scrolling capture).
    pub fn with_auto_command(mut self, command: Command) -> Self {
        self.auto_command = Some(command);
        self
    }

    /// The interface scale (the monitor's, 1.0 at 100 %): the toolbar grows with it, and the tools start as thick as
    /// they look at 100 %.
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(0.5, 4.0);
        self.set_width(DEFAULT_WIDTH * self.ui_scale);
    }

    /// The largest thickness, for this scale.
    pub fn max_width(&self) -> f32 {
        MAX_WIDTH * self.ui_scale
    }

    /// Shows the screens live (`true`) or frozen. Live, the annotations are drawn over nothing, and the host takes a
    /// fresh picture of the screens before it exports (see [`Session::replace_base`]).
    pub fn set_live(&mut self, live: bool) {
        self.live = live;
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    /// Takes `base` (a fresh picture of the same screens) instead of the frozen one. `false`, and nothing changes, when
    /// it is not the same size.
    pub fn replace_base(&mut self, base: RgbaImage) -> bool {
        if (base.width, base.height) != (self.base.width, self.base.height) {
            return false;
        }
        self.base = base;
        self.base_version += 1;
        true
    }

    /// Live, the part of the screens (whole image pixels) that annotations take their pixels from: the zone, while it
    /// holds a blur, a pixelation or a highlight. The host follows it, to draw the layers over it. `None` while nothing
    /// needs it, and while the zone is being moved or resized.
    pub fn live_area(&self) -> Option<(u32, u32, u32, u32)> {
        if !self.live || self.target == Some(Target::Selection) {
            return None;
        }
        let area = self.zone_px()?;
        let effect = |a: &Annotation| {
            matches!(a, Annotation::Blur { .. } | Annotation::Pixelate { .. } | Annotation::Highlight { .. })
        };
        let used = self.editor.doc.items.iter().any(|i| effect(&i.annotation))
            || self.editor.preview().is_some_and(|a| effect(&a));
        if !used {
            return None;
        }
        Some(area)
    }

    /// Removes every annotation (undo brings them back).
    pub fn clear(&mut self) {
        self.text_input = None;
        self.editor.clear();
    }

    pub fn ui_scale(&self) -> f32 {
        self.ui_scale
    }

    pub fn base(&self) -> &RgbaImage {
        &self.base
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// A press is being dragged (drawing, moving, resizing, annotating).
    pub fn in_gesture(&self) -> bool {
        self.target.is_some()
    }

    /// What a mere hover can change on screen: the cursor shape and the window under it.
    pub fn hover_state(&self) -> (CursorHint, Option<Rect>) {
        (self.cursor_hint(), self.selection.hover_window)
    }

    pub fn tool(&self) -> Option<Tool> {
        self.tool
    }

    /// `None` = adjust the zone (move, resize). Choosing a tool before a zone exists is allowed and takes effect once
    /// the zone is drawn.
    pub fn choose_tool(&mut self, tool: Option<Tool>) {
        self.text_input = None;
        if tool == Some(Tool::Eyedropper) && self.tool != tool {
            self.before_eyedropper = self.tool;
        }
        self.tool = tool;
        if let Some(t) = self.tool {
            self.editor.tool = t;
        }
    }

    pub fn set_color(&mut self, color: Color) {
        self.editor.set_color(color);
    }

    pub fn set_width(&mut self, width: f32) {
        let width = width.clamp(1.0, self.max_width());
        let scale = self.ui_scale;
        let settings = &mut self.editor.settings;
        settings.style.width = width;
        // One thickness control sizes every tool (defaults at width 4, at 100 %: 24 px text, 14 px markers, blur radius
        // 6, 10 px mosaic cells).
        settings.text_size = 8.0 * scale + 4.0 * width;
        settings.marker_radius = 10.0 * scale + width;
        settings.blur_radius = 1.5 * width;
        settings.pixelate_block = (2.5 * width).round().max(2.0) as u32;
    }

    pub fn set_filled(&mut self, filled: bool) {
        self.editor.settings.filled = filled;
    }

    fn route(&self, p: Point) -> Target {
        match (self.tool, self.selection.rect()) {
            (tool, Some(rect))
                if self.selection.is_settled() && self.selection.handle_at(p).is_none() && rect.contains(p) =>
            {
                // The zone tool moves an annotation when it is pressed on one, the zone else.
                if tool.is_some() || self.editor.doc.hit_test(p).is_some() { Target::Editor } else { Target::Selection }
            }
            _ => Target::Selection,
        }
    }

    /// The zone tool is in hand, or the selection one: annotations can be picked and moved.
    fn picking(&self) -> bool {
        matches!(self.tool, None | Some(Tool::Select))
    }

    /// The outline of the selected annotation (of its moved copy during a move), while annotations can be picked.
    fn selected(&self) -> Option<Rect> {
        if !self.picking() {
            return None;
        }
        if self.editor.hidden().is_some() {
            return self.editor.preview().map(|a| a.bounds());
        }
        let id = self.editor.selection()?;
        self.editor.doc.get(id).map(Annotation::bounds)
    }

    /// The size of what the tool in hand draws, in image pixels (the stroke's width, the marker's or the text's height,
    /// the blur's reach); `None` for tools that draw nothing.
    pub fn brush(&self) -> Option<f32> {
        let s = &self.editor.settings;
        Some(match self.tool? {
            Tool::Pen | Tool::Line | Tool::Arrow | Tool::Rect | Tool::Ellipse => s.style.width,
            Tool::Highlighter => crate::editor::highlight_width(s.style.width),
            Tool::Marker => 2.0 * s.marker_radius,
            Tool::Text => s.text_size,
            Tool::Blur => 2.0 * s.blur_radius,
            Tool::Pixelate => s.pixelate_block as f32,
            Tool::Select | Tool::Eyedropper => return None,
        })
    }

    /// Makes what the tools draw larger (`steps` > 0) or smaller, by steps that grow with the size (the mouse wheel).
    pub fn grow(&mut self, steps: i32) {
        let mut width = self.editor.settings.style.width;
        for _ in 0..steps.unsigned_abs() {
            let step = (width / 8.0).round().max(1.0);
            width += if steps > 0 { step } else { -step };
        }
        self.set_width(width);
    }

    pub fn pointer_down(&mut self, p: Point, mods: Modifiers) {
        self.cursor = p;
        self.text_input = None;
        let target = self.route(p);
        self.target = Some(target);
        match target {
            Target::Selection => {
                self.editor.deselect();
                // Starting a new zone leaves the tool and the zone-bound gesture state.
                if self.selection.rect().is_none_or(|r| !r.contains(p)) && self.selection.handle_at(p).is_none() {
                    self.tool = None;
                }
                self.selection.pointer_down(p);
            }
            Target::Editor => {
                if self.picking() {
                    self.editor.tool = Tool::Select;
                }
                match self.editor.pointer_down(p, mods, Some(&self.base)) {
                    Outcome::TextRequested(at) => {
                        self.text_input = Some(at);
                        self.target = None;
                    }
                    // The colour is taken: back to the tool it is for.
                    Outcome::Picked(_) => {
                        let back = self.before_eyedropper.take();
                        self.choose_tool(back);
                        self.target = None;
                    }
                    Outcome::Nothing => {}
                }
            }
        }
    }

    pub fn pointer_move(&mut self, p: Point, mods: Modifiers) {
        self.cursor = p;
        match self.target {
            Some(Target::Selection) | None => self.selection.pointer_move(p),
            Some(Target::Editor) => self.editor.pointer_move(p, mods),
        }
    }

    /// Ends the gesture. Returns a command when the zone just became final and an automatic command was requested (see
    /// [`Session::with_auto_command`]).
    pub fn pointer_up(&mut self, p: Point, mods: Modifiers) -> Option<Command> {
        self.cursor = p;
        match self.target.take() {
            Some(Target::Selection) => {
                self.selection.pointer_up(p);
                if self.selection.is_settled() {
                    return self.auto_command.take();
                }
            }
            Some(Target::Editor) => self.editor.pointer_up(p, mods),
            None => {}
        }
        None
    }

    /// Ends a gesture the window could not see the end of (it lost the mouse) where the pointer last was. A zone
    /// finished this way waits for a real release to run its automatic command.
    pub fn interrupt(&mut self) {
        if self.target.is_none() {
            return;
        }
        let auto = self.auto_command.take();
        let _ = self.pointer_up(self.cursor, Modifiers::default());
        self.auto_command = auto;
    }

    /// Validates the text typed in the field opened by the Text tool.
    pub fn commit_text(&mut self, text: &str) {
        if let Some(at) = self.text_input.take() {
            self.editor.add_text(at, text);
        }
    }

    pub fn cancel_text(&mut self) {
        self.text_input = None;
    }

    #[cfg(test)]
    pub fn is_typing(&self) -> bool {
        self.text_input.is_some()
    }

    pub fn key(&mut self, input: KeyInput) -> Option<Command> {
        let KeyInput { key, ctrl, shift } = input;
        match key {
            Key::Escape if self.text_input.is_some() => {
                self.text_input = None;
                None
            }
            Key::Escape => Some(Command::Close),
            Key::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'c' if shift => Some(Command::CopyText),
                'c' => Some(Command::Copy),
                's' if shift => Some(Command::SaveAs),
                's' => Some(Command::Save),
                'z' if shift => {
                    self.editor.redo();
                    None
                }
                'z' => {
                    self.editor.undo();
                    None
                }
                'y' => {
                    self.editor.redo();
                    None
                }
                'a' => {
                    self.selection.select_all();
                    None
                }
                _ => None,
            },
            Key::Delete => {
                self.editor.delete_selected();
                None
            }
            Key::Left | Key::Right | Key::Up | Key::Down => {
                let step = if shift { 10.0 } else { 1.0 };
                let (dx, dy) = match key {
                    Key::Left => (-step, 0.0),
                    Key::Right => (step, 0.0),
                    Key::Up => (0.0, -step),
                    _ => (0.0, step),
                };
                if self.picking() {
                    self.editor.nudge(dx, dy);
                }
                None
            }
            _ => None,
        }
    }

    /// Takes the whole image as the zone.
    pub fn select_all(&mut self) {
        self.selection.select_all();
    }

    pub fn undo(&mut self) {
        self.editor.undo();
    }

    pub fn redo(&mut self) {
        self.editor.redo();
    }

    /// The selected zone, in image pixels.
    pub fn zone(&self) -> Option<Rect> {
        self.selection.rect()
    }

    /// The final image: the zone with its annotations. `None` while there is no zone.
    pub fn export(&self) -> Option<RgbaImage> {
        let zone = self.selection.rect()?;
        render_region(&self.base, self.editor.doc.items.iter().map(|i| &i.annotation), &zone)
    }

    /// Where the pointer is, to pick the cursor shape.
    pub fn cursor_hint(&self) -> CursorHint {
        match (self.tool, self.selection.rect()) {
            (Some(t), Some(r)) if r.contains(self.cursor) && self.selection.handle_at(self.cursor).is_none() => {
                if t == Tool::Select { CursorHint::Default } else { CursorHint::Crosshair }
            }
            _ => self.selection.cursor_at(self.cursor),
        }
    }

    /// The annotations and the gesture in progress, bottom to top (see [`View::layers`]).
    fn layers(&self) -> Rc<Vec<Annotation>> {
        let hidden = self.editor.hidden();
        let preview = self.editor.preview();
        let key = (self.editor.revision(), hidden, preview);
        let mut last = self.layers.borrow_mut();
        if last.1.as_ref() == Some(&key) {
            return last.0.clone();
        }
        let layers: Vec<Annotation> = self
            .editor
            .doc
            .items
            .iter()
            .filter(|i| Some(i.id) != hidden)
            .map(|i| i.annotation.clone())
            .chain(key.2.clone())
            .collect();
        *last = (Rc::new(layers), Some(key));
        last.0.clone()
    }

    fn layers_area(&self, layers: &Rc<Vec<Annotation>>) -> Option<Area> {
        let mut kept = self.area.borrow_mut();
        let zone = self.zone_px();
        let mut coverage = self.coverage.borrow_mut();
        if !Rc::ptr_eq(&coverage.0, layers) {
            *coverage = (layers.clone(), layers.iter().map(Annotation::bounds).reduce(|a, b| a.union(&b)));
        }
        let needed = zone.and_then(|zone| Self::needed_area(zone, coverage.1));
        let (Some(zone), Some(needed)) = (zone, needed) else {
            *kept = None;
            return None;
        };
        let area = match *kept {
            Some((z, last)) if z == zone => grown(last, needed, zone),
            _ => needed,
        };
        *kept = Some((zone, area));
        Some(area)
    }

    /// What `layers` cover in `zone`, by blocks.
    fn needed_area((zx, zy, zw, zh): Area, covered: Option<Rect>) -> Option<Area> {
        let covered = covered?.inflate(LAYER_MARGIN);
        let block = LAYER_BLOCK as f32;
        let x0 = ((covered.x / block).floor() * block).max(zx as f32) as u32;
        let y0 = ((covered.y / block).floor() * block).max(zy as f32) as u32;
        let x1 = (((covered.x + covered.w) / block).ceil() * block).min((zx + zw) as f32) as u32;
        let y1 = (((covered.y + covered.h) / block).ceil() * block).min((zy + zh) as f32) as u32;
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
    }

    /// The zone in whole image pixels (x, y, width, height), rounded outwards and clamped.
    fn zone_px(&self) -> Option<(u32, u32, u32, u32)> {
        let zone = self.selection.rect()?;
        let x0 = (zone.x.floor().max(0.0) as u32).min(self.base.width);
        let y0 = (zone.y.floor().max(0.0) as u32).min(self.base.height);
        let x1 = ((zone.x + zone.w).ceil().max(0.0) as u32).min(self.base.width);
        let y1 = ((zone.y + zone.h).ceil().max(0.0) as u32).min(self.base.height);
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
    }

    pub fn view(&self) -> View {
        let zone = self.selection.rect();
        let layers = self.layers();
        let layers_area = self.layers_area(&layers);
        let settled = self.selection.is_settled();
        let settings = &self.editor.settings;
        let bar_size = (TOOLBAR_SIZE.0 * self.ui_scale, TOOLBAR_SIZE.1 * self.ui_scale);
        let toolbar =
            zone.filter(|_| settled).map(|z| place_toolbar(&z, bar_size, |p| self.screen_at(p), GAP * self.ui_scale));
        View {
            selection: zone,
            settled,
            hover_window: self.selection.hover_window,
            size_label: zone.map(|z| {
                let text = format!("{} × {}", z.w.round() as u32, z.h.round() as u32);
                let at = place_label(
                    &z,
                    (LABEL_SIZE.0 * self.ui_scale, LABEL_SIZE.1 * self.ui_scale),
                    toolbar.map(|b| b.rect(bar_size)),
                    |p| self.screen_at(p),
                    LABEL_GAP * self.ui_scale,
                );
                (text, at)
            }),
            toolbar,
            drawing: self.target == Some(Target::Editor)
                && self.layers.borrow().1.as_ref().is_some_and(|key| key.2.is_some()),
            selected: self.selected(),
            brush: self.brush(),
            pointer: self.cursor,
            live: self.live,
            max_width: self.max_width(),
            layers,
            layers_area,
            base_version: self.base_version,
            cursor: self.cursor_hint(),
            tool: self.tool,
            color: settings.style.color,
            recent_colors: self.editor.recent_colors().to_vec(),
            width: settings.style.width,
            filled: settings.filled,
            can_undo: self.editor.can_undo(),
            can_redo: self.editor.can_redo(),
            can_clear: !self.editor.doc.items.is_empty(),
            text_input: self.text_input,
            text_size: settings.text_size,
            dim: self.dim,
        }
    }
}

/// `last` if it holds `needed`; otherwise both, one more block out on each side where `needed` goes past `last`, inside
/// `zone`.
fn grown(last: Area, needed: Area, (zx, zy, zw, zh): Area) -> Area {
    let block = LAYER_BLOCK;
    let (lx1, ly1) = (last.0 + last.2, last.1 + last.3);
    let (nx1, ny1) = (needed.0 + needed.2, needed.1 + needed.3);
    let x0 = if needed.0 < last.0 { needed.0.saturating_sub(block).max(zx) } else { last.0 };
    let y0 = if needed.1 < last.1 { needed.1.saturating_sub(block).max(zy) } else { last.1 };
    let x1 = if nx1 > lx1 { (nx1 + block).min(zx + zw) } else { lx1 };
    let y1 = if ny1 > ly1 { (ny1 + block).min(zy + zh) } else { ly1 };
    (x0, y0, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests;
