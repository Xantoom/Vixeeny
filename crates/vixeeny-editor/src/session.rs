// SPDX-License-Identifier: GPL-3.0-or-later
//! One Print Screen session (plan 5.3): the frozen image, the zone, the tools and the commands.
//! Everything the UI needs is read from [`Session::view`]; everything the user does goes
//! through the `pointer_*`, `key` and `choose_*` methods. No UI toolkit is involved.

use crate::editor::{Editor, Modifiers, Outcome, Tool};
use crate::geometry::{Point, Rect};
use crate::model::Color;
use crate::render::{RgbaImage, render_region};
use crate::selection::{CursorHint, Selection, magnifier_position, magnifier_source, place_beside};

/// Size of the toolbar, in image pixels (the UI lays out its content to fit).
pub const TOOLBAR_SIZE: (f32, f32) = (700.0, 88.0);
/// Source pixels shown by the magnifier, per side, and the on-screen zoom factor.
pub const MAGNIFIER_SIDE: u32 = 11;
pub const MAGNIFIER_ZOOM: f32 = 10.0;
/// Space around the toolbar and the magnifier.
const GAP: f32 = 8.0;
const MAGNIFIER_OFFSET: f32 = 24.0;

/// What the user asked the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Copy the result to the clipboard, then close.
    Copy,
    /// Save with the default folder and format.
    Save,
    /// Ask for a folder and a format.
    SaveAs,
    /// Run OCR on the zone.
    Ocr,
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
    /// The zone with its annotations (and the gesture in progress) drawn on it. `None` when
    /// there is nothing to draw: show the frozen image as is.
    pub annotated: Option<(RgbaImage, Point)>,
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
    /// Scale of the interface (the window's scale factor); the toolbar and magnifier grow with it.
    ui_scale: f32,
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
            ui_scale: 1.0,
        }
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
        self.editor.settings.style.width = width.clamp(1.0, 40.0);
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

    pub fn pointer_up(&mut self, p: Point, mods: Modifiers) {
        self.cursor = p;
        match self.target.take() {
            Some(Target::Selection) => self.selection.pointer_up(p),
            Some(Target::Editor) => self.editor.pointer_up(p, mods),
            None => {}
        }
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
                't' => Some(Command::Ocr),
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

    pub fn undo(&mut self) {
        self.editor.undo();
    }

    pub fn redo(&mut self) {
        self.editor.redo();
    }

    /// The final image: the zone with its annotations. `None` while there is no zone.
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

    fn annotated(&self, zone: &Rect) -> Option<(RgbaImage, Point)> {
        let preview = self.editor.preview();
        if self.editor.doc.items.is_empty() && preview.is_none() {
            return None;
        }
        let hidden = self.editor.hidden();
        let items = self
            .editor
            .doc
            .items
            .iter()
            .filter(|i| Some(i.id) != hidden)
            .map(|i| &i.annotation)
            .chain(preview.as_ref());
        let image = render_region(&self.base, items, zone)?;
        // `render_region` rounds the zone outwards; the image sits at the rounded corner.
        Some((
            image,
            Point::new(zone.x.floor().max(0.0), zone.y.floor().max(0.0)),
        ))
    }

    fn magnifier(&self) -> Option<MagnifierView> {
        let bounds = Rect::new(0.0, 0.0, self.base.width as f32, self.base.height as f32);
        let src = magnifier_source(self.cursor, MAGNIFIER_SIDE, &bounds);
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
        let bounds = Rect::new(0.0, 0.0, self.base.width as f32, self.base.height as f32);
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
            magnifier: if zone.is_none() || !settled {
                self.magnifier()
            } else {
                None
            },
            toolbar: zone.filter(|_| settled).map(|z| {
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
            dim: self.dim,
        }
    }
}

#[cfg(test)]
mod tests;
