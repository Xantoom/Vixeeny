// SPDX-License-Identifier: GPL-3.0-or-later
//! One Print Screen session (plan 5.3): the frozen image, the zone, the tools and the commands.
//! Everything the UI needs is read from [`Session::view`]; everything the user does goes
//! through the `pointer_*`, `key` and `choose_*` methods. No UI toolkit is involved.

use std::cell::RefCell;
use std::rc::Rc;

use crate::editor::{Editor, Modifiers, Outcome, Tool};
use crate::geometry::{Point, Rect};
use crate::model::{Annotation, Color};
use crate::render::{RgbaImage, render_region};
use crate::selection::{CursorHint, Selection, magnifier_position, magnifier_source, place_beside};

/// Size of the toolbar, in image pixels (the UI lays out its content to fit).
pub const TOOLBAR_SIZE: (f32, f32) = (779.0, 46.0);
/// Source pixels shown by the magnifier, per side, and the on-screen zoom factor.
pub const MAGNIFIER_SIDE: u32 = 11;
pub const MAGNIFIER_ZOOM: f32 = 10.0;
/// Space around the toolbar and the magnifier.
const GAP: f32 = 8.0;
const MAGNIFIER_OFFSET: f32 = 24.0;
/// Drawn around the annotations' bounds: miter joins and anti-aliasing reach a little past them.
const DRAW_MARGIN: f32 = 8.0;

/// What the user asked the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Copy the result to the clipboard, then close.
    Copy,
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

#[derive(Debug, Clone, PartialEq)]
pub struct MagnifierView {
    /// The `MAGNIFIER_SIDE`² source pixels, to be shown `MAGNIFIER_ZOOM` times larger with no
    /// smoothing.
    pub pixels: RgbaImage,
    pub position: Point,
    /// Side of the magnified square on screen, in pixels.
    pub size: f32,
    /// Colour under the cursor, `#rrggbb`.
    pub hex: String,
    pub color: Color,
}

/// Everything to display, recomputed after each input.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub selection: Option<Rect>,
    /// The zone is final for now (not being drawn): handles and toolbar are shown.
    pub settled: bool,
    pub hover_window: Option<Rect>,
    pub size_label: Option<(String, Point)>,
    pub magnifier: Option<MagnifierView>,
    pub toolbar: Option<Point>,
    /// The part of the zone that annotations (and the gesture in progress) cover, drawn, and
    /// where it goes. `None` when there is nothing to draw: show the frozen image as is. The same
    /// `Rc` comes back while nothing changes, so the host can skip uploading it again.
    pub annotated: Option<(Rc<RgbaImage>, Point)>,
    pub cursor: CursorHint,
    pub tool: Option<Tool>,
    pub color: Color,
    pub recent_colors: Vec<Color>,
    pub width: f32,
    pub filled: bool,
    pub can_undo: bool,
    pub can_redo: bool,
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
    target: Option<Target>,
    cursor: Point,
    text_input: Option<Point>,
    /// Veil opacity over the image outside the zone (0.0–1.0).
    pub dim: f32,
    /// Command issued once, as soon as the first zone is settled (the scrolling capture: no toolbar step).
    auto_command: Option<Command>,
    /// Scale of the interface (the window's scale factor); the toolbar and magnifier grow with it.
    ui_scale: f32,
    /// The monitors, in image pixels: the toolbar and the magnifier stay on one of them.
    screens: Vec<Rect>,
    /// The last drawing of the annotations, with what it was drawn from.
    drawn: RefCell<Option<Drawn>>,
}

/// A drawing of the annotations: what was drawn, where, and the result.
#[derive(Debug)]
struct Drawn {
    layers: Vec<Annotation>,
    region: Rect,
    image: Rc<RgbaImage>,
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
            target: None,
            cursor: Point::default(),
            text_input: None,
            dim: 0.4,
            auto_command: None,
            ui_scale: 1.0,
            screens: vec![bounds],
            drawn: RefCell::new(None),
        }
    }

    /// The monitors the image spans (image pixels), so that the toolbar and the magnifier never
    /// straddle two of them.
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

    /// Issues `command` as soon as the first zone is validated (the scrolling capture).
    pub fn with_auto_command(mut self, command: Command) -> Self {
        self.auto_command = Some(command);
        self
    }

    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(0.5, 4.0);
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

    /// `None` = adjust the zone (move, resize). Choosing a tool before a zone exists is allowed
    /// and takes effect once the zone is drawn.
    pub fn choose_tool(&mut self, tool: Option<Tool>) {
        self.text_input = None;
        // Crop is the zone itself: adjusting it is the crop tool.
        self.tool = tool.filter(|t| *t != Tool::Crop);
        if let Some(t) = self.tool {
            self.editor.tool = t;
        }
    }

    pub fn set_color(&mut self, color: Color) {
        self.editor.set_color(color);
    }

    pub fn set_width(&mut self, width: f32) {
        let width = width.clamp(1.0, 40.0);
        let settings = &mut self.editor.settings;
        settings.style.width = width;
        // One thickness control sizes every tool (defaults at width 4: 24 px text, 14 px markers,
        // blur radius 6, 10 px mosaic cells).
        settings.text_size = 8.0 + 4.0 * width;
        settings.marker_radius = 10.0 + width;
        settings.blur_radius = 1.5 * width;
        settings.pixelate_block = (2.5 * width).round().max(2.0) as u32;
    }

    pub fn set_filled(&mut self, filled: bool) {
        self.editor.settings.filled = filled;
    }

    fn route(&self, p: Point) -> Target {
        match (self.tool, self.selection.rect()) {
            (Some(_), Some(rect)) if self.selection.is_settled() => {
                if self.selection.handle_at(p).is_some() {
                    Target::Selection
                } else if rect.contains(p) {
                    Target::Editor
                } else {
                    Target::Selection
                }
            }
            _ => Target::Selection,
        }
    }

    pub fn pointer_down(&mut self, p: Point, mods: Modifiers) {
        self.cursor = p;
        self.text_input = None;
        let target = self.route(p);
        self.target = Some(target);
        match target {
            Target::Selection => {
                // Starting a new zone leaves the tool and the zone-bound gesture state.
                if self.selection.rect().is_none_or(|r| !r.contains(p))
                    && self.selection.handle_at(p).is_none()
                {
                    self.tool = None;
                }
                self.selection.pointer_down(p);
            }
            Target::Editor => {
                if let Outcome::TextRequested(at) =
                    self.editor.pointer_down(p, mods, Some(&self.base))
                {
                    self.text_input = Some(at);
                    self.target = None;
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

    /// Ends the gesture. Returns a command when the zone just became final and an automatic
    /// command was requested (see [`Session::with_auto_command`]).
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

    /// Validates the text typed in the field opened by the Text tool.
    pub fn commit_text(&mut self, text: &str) {
        if let Some(at) = self.text_input.take() {
            self.editor.add_text(at, text);
        }
    }

    pub fn cancel_text(&mut self) {
        self.text_input = None;
    }

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
                if self.tool == Some(Tool::Select) {
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

    /// The final image: the zone with its annotations. `None` while there is no zone.
    /// The selected zone, in image pixels.
    pub fn zone(&self) -> Option<Rect> {
        self.selection.rect()
    }

    pub fn export(&self) -> Option<RgbaImage> {
        let zone = self.selection.rect()?;
        render_region(
            &self.base,
            self.editor.doc.items.iter().map(|i| &i.annotation),
            &zone,
        )
    }

    /// Where the pointer is, to pick the cursor shape.
    pub fn cursor_hint(&self) -> CursorHint {
        match (self.tool, self.selection.rect()) {
            (Some(t), Some(r))
                if r.contains(self.cursor) && self.selection.handle_at(self.cursor).is_none() =>
            {
                if t == Tool::Select {
                    CursorHint::Default
                } else {
                    CursorHint::Crosshair
                }
            }
            _ => self.selection.cursor_at(self.cursor),
        }
    }

    fn annotated(&self, zone: &Rect) -> Option<(Rc<RgbaImage>, Point)> {
        let preview = self.editor.preview();
        if self.editor.doc.items.is_empty() && preview.is_none() {
            return None;
        }
        let hidden = self.editor.hidden();
        let layers: Vec<Annotation> = self
            .editor
            .doc
            .items
            .iter()
            .filter(|i| Some(i.id) != hidden)
            .map(|i| i.annotation.clone())
            .chain(preview)
            .collect();
        // Only what the annotations cover (with a margin for joins and anti-aliasing): the
        // frozen image already shows the rest, and a small image is quick to draw and upload.
        let covered = layers
            .iter()
            .map(Annotation::bounds)
            .reduce(|a, b| a.union(&b))?
            .inflate(DRAW_MARGIN);
        let region = covered.intersect(zone)?;
        let mut drawn = self.drawn.borrow_mut();
        let image = match drawn.as_ref() {
            Some(d) if d.region == region && d.layers == layers => d.image.clone(),
            _ => {
                let image = Rc::new(render_region(&self.base, layers.iter(), &region)?);
                *drawn = Some(Drawn {
                    layers,
                    region,
                    image: image.clone(),
                });
                image
            }
        };
        // `render_region` rounds the region outwards; the image sits at the rounded corner.
        Some((
            image,
            Point::new(region.x.floor().max(0.0), region.y.floor().max(0.0)),
        ))
    }

    fn magnifier(&self) -> Option<MagnifierView> {
        let image = Rect::new(0.0, 0.0, self.base.width as f32, self.base.height as f32);
        let bounds = self.screen_at(self.cursor);
        let src = magnifier_source(self.cursor, MAGNIFIER_SIDE, &image);
        let pixels = render_region(&self.base, std::iter::empty(), &src)?;
        let (px, py) = (
            (self.cursor.x.max(0.0) as u32).min(self.base.width - 1),
            (self.cursor.y.max(0.0) as u32).min(self.base.height - 1),
        );
        let color = self.base.pixel(px, py).with_alpha(255);
        let scale = self.ui_scale;
        let size = MAGNIFIER_SIDE as f32 * MAGNIFIER_ZOOM * scale;
        Some(MagnifierView {
            pixels,
            size,
            position: magnifier_position(
                self.cursor,
                (size, size + 28.0 * scale),
                &bounds,
                MAGNIFIER_OFFSET * scale,
            ),
            hex: color.hex(),
            color,
        })
    }

    pub fn view(&self) -> View {
        let zone = self.selection.rect();
        let settled = self.selection.is_settled();
        let settings = &self.editor.settings;
        View {
            selection: zone,
            settled,
            hover_window: self.selection.hover_window,
            size_label: zone.map(|z| {
                let text = format!("{} × {}", z.w.round() as u32, z.h.round() as u32);
                let label = 26.0 * self.ui_scale;
                let y = if z.y >= label { z.y - label } else { z.y + 4.0 };
                (text, Point::new(z.x, y))
            }),
            // Only while an edge of the zone is being placed: a calm frozen screen otherwise.
            magnifier: if zone.is_some() && self.selection.is_placing_an_edge() {
                self.magnifier()
            } else {
                None
            },
            toolbar: zone.filter(|_| settled).map(|z| {
                // On the monitor of the zone's bottom-right corner, where the bar goes.
                let bounds = self.screen_at(Point::new(z.right() - 1.0, z.bottom() - 1.0));
                place_beside(
                    &z,
                    (
                        TOOLBAR_SIZE.0 * self.ui_scale,
                        TOOLBAR_SIZE.1 * self.ui_scale,
                    ),
                    &bounds,
                    GAP * self.ui_scale,
                )
            }),
            annotated: zone.and_then(|z| self.annotated(&z)),
            cursor: self.cursor_hint(),
            tool: self.tool,
            color: settings.style.color,
            recent_colors: self.editor.recent_colors().to_vec(),
            width: settings.style.width,
            filled: settings.filled,
            can_undo: self.editor.can_undo(),
            can_redo: self.editor.can_redo(),
            text_input: self.text_input,
            text_size: settings.text_size,
            dim: self.dim,
        }
    }
}

#[cfg(test)]
mod tests;
