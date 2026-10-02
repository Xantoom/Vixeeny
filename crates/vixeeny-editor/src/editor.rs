// SPDX-License-Identifier: GPL-3.0-or-later
//! Tool interaction: turns pointer events into edits of the document (plan 5.3). The UI feeds
//! pointer positions in image pixels and draws [`Editor::preview`] while a gesture is running.

use crate::geometry::{Point, Rect, snap_angle, square_corner};
use crate::history::{Edit, History};
use crate::model::{Annotation, AnnotationId, Color, Document, Item, Style};
use crate::render::RgbaImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Pen,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Highlighter,
    Blur,
    Pixelate,
    Marker,
    Crop,
    Eyedropper,
}

/// Options shared by the tools.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSettings {
    pub style: Style,
    pub filled: bool,
    pub text_size: f32,
    pub text_background: Option<Color>,
    pub blur_radius: f32,
    pub pixelate_block: u32,
    /// Opacity of the highlighter, 0–255.
    pub highlight_alpha: u8,
    pub marker_radius: f32,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            style: Style::default(),
            filled: false,
            text_size: 24.0,
            text_background: None,
            blur_radius: 6.0,
            pixelate_block: 10,
            highlight_alpha: 110,
            marker_radius: 14.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
}

/// What the UI must do after a pointer press.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Nothing,
    /// Open a text field at this point, then call [`Editor::add_text`].
    TextRequested(Point),
    /// The pipette picked this colour.
    Picked(Color),
}

#[derive(Debug, Clone)]
enum Drag {
    Draw {
        start: Point,
        points: Vec<Point>,
        current: Point,
        shift: bool,
    },
    Move {
        id: AnnotationId,
        start: Point,
        original: Annotation,
        delta: (f32, f32),
    },
}

/// Smallest drag, in pixels, that creates a shape.
const MIN_DRAG: f32 = 3.0;
const RECENT_COLORS: usize = 8;

pub struct Editor {
    pub doc: Document,
    pub tool: Tool,
    pub settings: ToolSettings,
    history: History,
    selection: Option<AnnotationId>,
    drag: Option<Drag>,
    recent: Vec<Color>,
}

impl Editor {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            doc: Document::new(width, height),
            tool: Tool::Rect,
            settings: ToolSettings::default(),
            history: History::default(),
            selection: None,
            drag: None,
            recent: Vec::new(),
        }
    }

    pub fn selection(&self) -> Option<AnnotationId> {
        self.selection
    }

    pub fn recent_colors(&self) -> &[Color] {
        &self.recent
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Sets the drawing colour and remembers it among the last used.
    pub fn set_color(&mut self, color: Color) {
        self.settings.style.color = color;
        self.recent.retain(|c| *c != color);
        self.recent.insert(0, color);
        self.recent.truncate(RECENT_COLORS);
    }

    fn add(&mut self, annotation: Annotation) -> AnnotationId {
        let id = self.doc.fresh_id();
        let index = self.doc.items.len();
        self.history.commit(
            &mut self.doc,
            Edit::Add {
                index,
                item: Item { id, annotation },
            },
        );
        id
    }

    fn clamp_to_image(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(0.0, self.doc.size.0 as f32),
            p.y.clamp(0.0, self.doc.size.1 as f32),
        )
    }

    pub fn pointer_down(&mut self, p: Point, mods: Modifiers, base: Option<&RgbaImage>) -> Outcome {
        self.drag = None;
        match self.tool {
            Tool::Select => {
                self.selection = self.doc.hit_test(p);
                if let Some((id, original)) = self
                    .selection
                    .and_then(|id| self.doc.get(id).map(|a| (id, a.clone())))
                {
                    self.drag = Some(Drag::Move {
                        id,
                        start: p,
                        original,
                        delta: (0.0, 0.0),
                    });
                }
                Outcome::Nothing
            }
            Tool::Text => Outcome::TextRequested(p),
            Tool::Marker => {
                let number = self.doc.next_marker_number();
                let (color, radius) = (self.settings.style.color, self.settings.marker_radius);
                let id = self.add(Annotation::Marker {
                    center: p,
                    number,
                    color,
                    radius,
                });
                self.selection = Some(id);
                Outcome::Nothing
            }
            Tool::Eyedropper => base
                .and_then(|img| self.pick_color(img, p))
                .map_or(Outcome::Nothing, Outcome::Picked),
            _ => {
                let p = self.clamp_to_image(p);
                self.drag = Some(Drag::Draw {
                    start: p,
                    points: vec![p],
                    current: p,
                    shift: mods.shift,
                });
                Outcome::Nothing
            }
        }
    }

    pub fn pointer_move(&mut self, p: Point, mods: Modifiers) {
        let clamp = self.clamp_to_image(p);
        match &mut self.drag {
            Some(Drag::Draw {
                points,
                current,
                shift,
                ..
            }) => {
                *current = clamp;
                *shift = mods.shift;
                if points.last().is_none_or(|last| last.distance(clamp) >= 1.0) {
                    points.push(clamp);
                }
            }
            Some(Drag::Move { start, delta, .. }) => *delta = (p.x - start.x, p.y - start.y),
            None => {}
        }
    }

    pub fn pointer_up(&mut self, p: Point, mods: Modifiers) {
        self.pointer_move(p, mods);
        match self.drag.take() {
            Some(drag @ Drag::Draw { .. }) => self.finish_draw(drag),
            Some(Drag::Move {
                id,
                original,
                delta,
                ..
            }) if delta != (0.0, 0.0) => {
                let new = original.translated(delta.0, delta.1);
                let old = original;
                self.history
                    .commit(&mut self.doc, Edit::Replace { id, old, new });
            }
            Some(Drag::Move { .. }) => {}
            None => {}
        }
    }

    /// The annotation being drawn or moved, to draw over the image while the button is down.
    pub fn preview(&self) -> Option<Annotation> {
        match self.drag.as_ref()? {
            Drag::Draw {
                start,
                points,
                current,
                shift,
            } => self.build(*start, points, *current, *shift),
            Drag::Move {
                original, delta, ..
            } => Some(original.translated(delta.0, delta.1)),
        }
    }

    /// The annotation hidden while its moved copy is previewed.
    pub fn hidden(&self) -> Option<AnnotationId> {
        match self.drag.as_ref()? {
            Drag::Move { id, .. } => Some(*id),
            Drag::Draw { .. } => None,
        }
    }

    /// The rectangle being cropped, while the Crop tool drags.
    pub fn crop_preview(&self) -> Option<Rect> {
        match self.drag.as_ref()? {
            Drag::Draw {
                start,
                current,
                shift,
                ..
            } if self.tool == Tool::Crop => Some(self.drag_rect(*start, *current, *shift)),
            _ => None,
        }
    }

    fn drag_rect(&self, start: Point, current: Point, shift: bool) -> Rect {
        let end = if shift {
            square_corner(start, current)
        } else {
            current
        };
        Rect::from_corners(start, self.clamp_to_image(end))
    }

    fn build(
        &self,
        start: Point,
        points: &[Point],
        current: Point,
        shift: bool,
    ) -> Option<Annotation> {
        let s = &self.settings;
        let big_enough = start.distance(current) >= MIN_DRAG;
        match self.tool {
            Tool::Pen => Some(Annotation::Pen {
                points: points.to_vec(),
                style: s.style,
            }),
            Tool::Highlighter => Some(Annotation::Highlight {
                points: points.to_vec(),
                color: s.style.color.with_alpha(s.highlight_alpha),
                width: (s.style.width * 4.0).max(12.0),
            }),
            Tool::Line | Tool::Arrow if big_enough => {
                let to = if shift {
                    snap_angle(start, current, 15.0)
                } else {
                    current
                };
                let to = self.clamp_to_image(to);
                Some(if self.tool == Tool::Line {
                    Annotation::Line {
                        from: start,
                        to,
                        style: s.style,
                    }
                } else {
                    Annotation::Arrow {
                        from: start,
                        to,
                        style: s.style,
                    }
                })
            }
            Tool::Rect | Tool::Ellipse | Tool::Blur | Tool::Pixelate if big_enough => {
                let rect = self.drag_rect(start, current, shift);
                Some(match self.tool {
                    Tool::Rect => Annotation::Rect {
                        rect,
                        style: s.style,
                        filled: s.filled,
                    },
                    Tool::Ellipse => Annotation::Ellipse {
                        rect,
                        style: s.style,
                        filled: s.filled,
                    },
                    Tool::Blur => Annotation::Blur {
                        rect,
                        radius: s.blur_radius,
                    },
                    _ => Annotation::Pixelate {
                        rect,
                        block: s.pixelate_block,
                    },
                })
            }
            _ => None,
        }
    }

    /// Commits the gesture (called by `pointer_up`).
    fn finish_draw(&mut self, drag: Drag) {
        let Drag::Draw {
            start,
            points,
            current,
            shift,
        } = drag
        else {
            return;
        };
        if self.tool == Tool::Crop {
            let rect = self.drag_rect(start, current, shift);
            if rect.w >= MIN_DRAG && rect.h >= MIN_DRAG {
                let old = self.doc.crop;
                self.history.commit(
                    &mut self.doc,
                    Edit::Crop {
                        old,
                        new: Some(rect),
                    },
                );
            }
        } else if let Some(a) = self.build(start, &points, current, shift) {
            let id = self.add(a);
            self.selection = Some(id);
        }
    }

    /// Adds a text annotation (after [`Outcome::TextRequested`]). Empty text adds nothing.
    pub fn add_text(&mut self, pos: Point, text: &str) -> Option<AnnotationId> {
        if text.trim().is_empty() {
            return None;
        }
        let s = &self.settings;
        let a = Annotation::Text {
            pos,
            text: text.to_owned(),
            size: s.text_size,
            color: s.style.color,
            background: s.text_background,
        };
        let id = self.add(a);
        self.selection = Some(id);
        Some(id)
    }

    /// Replaces the text of an existing text annotation.
    pub fn edit_text(&mut self, id: AnnotationId, text: &str) {
        let Some(
            old @ Annotation::Text {
                pos,
                size,
                color,
                background,
                ..
            },
        ) = self.doc.get(id).cloned()
        else {
            return;
        };
        let new = Annotation::Text {
            pos,
            text: text.to_owned(),
            size,
            color,
            background,
        };
        self.history
            .commit(&mut self.doc, Edit::Replace { id, old, new });
    }

    pub fn delete_selected(&mut self) {
        let Some(id) = self.selection.take() else {
            return;
        };
        if let Some(index) = self.doc.items.iter().position(|i| i.id == id) {
            let item = self.doc.items[index].clone();
            self.history
                .commit(&mut self.doc, Edit::Remove { index, item });
        }
    }

    /// Moves the selection by whole pixels (arrow keys).
    pub fn nudge(&mut self, dx: f32, dy: f32) {
        let Some(id) = self.selection else { return };
        if let Some(old) = self.doc.get(id).cloned() {
            let new = old.translated(dx, dy);
            self.history
                .commit(&mut self.doc, Edit::Replace { id, old, new });
        }
    }

    pub fn clear_crop(&mut self) {
        if let Some(old) = self.doc.crop {
            self.history.commit(
                &mut self.doc,
                Edit::Crop {
                    old: Some(old),
                    new: None,
                },
            );
        }
    }

    pub fn undo(&mut self) {
        self.drag = None;
        self.history.undo(&mut self.doc);
        self.drop_stale_selection();
    }

    pub fn redo(&mut self) {
        self.drag = None;
        self.history.redo(&mut self.doc);
        self.drop_stale_selection();
    }

    fn drop_stale_selection(&mut self) {
        if self.selection.is_some_and(|id| self.doc.get(id).is_none()) {
            self.selection = None;
        }
    }

    /// Colour of the base image under `p` (the pipette also reads the image, not annotations).
    pub fn pick_color(&mut self, base: &RgbaImage, p: Point) -> Option<Color> {
        if p.x < 0.0 || p.y < 0.0 || p.x >= base.width as f32 || p.y >= base.height as f32 {
            return None;
        }
        let c = base.pixel(p.x as u32, p.y as u32).with_alpha(255);
        self.set_color(c);
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO: Modifiers = Modifiers { shift: false };
    const SHIFT: Modifiers = Modifiers { shift: true };

    fn p(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }

    fn drag(ed: &mut Editor, from: Point, to: Point, mods: Modifiers) {
        ed.pointer_down(from, mods, None);
        ed.pointer_move(p((from.x + to.x) / 2.0, (from.y + to.y) / 2.0), mods);
        ed.pointer_up(to, mods);
    }

    fn only(ed: &Editor) -> &Annotation {
        assert_eq!(ed.doc.items.len(), 1, "{:?}", ed.doc.items);
        &ed.doc.items[0].annotation
    }

    #[test]
    fn every_drawing_tool_creates_its_annotation() {
        type Check = fn(&Annotation) -> bool;
        let cases: [(Tool, Check); 8] = [
            (Tool::Pen, |a| matches!(a, Annotation::Pen { .. })),
            (Tool::Line, |a| matches!(a, Annotation::Line { .. })),
            (Tool::Arrow, |a| matches!(a, Annotation::Arrow { .. })),
            (Tool::Rect, |a| matches!(a, Annotation::Rect { .. })),
            (Tool::Ellipse, |a| matches!(a, Annotation::Ellipse { .. })),
            (Tool::Highlighter, |a| {
                matches!(a, Annotation::Highlight { .. })
            }),
            (Tool::Blur, |a| matches!(a, Annotation::Blur { .. })),
            (Tool::Pixelate, |a| matches!(a, Annotation::Pixelate { .. })),
        ];
        for (tool, check) in cases {
            let mut ed = Editor::new(200, 100);
            ed.tool = tool;
            drag(&mut ed, p(10.0, 10.0), p(90.0, 60.0), NO);
            assert!(check(only(&ed)), "{tool:?}");
            assert_eq!(ed.selection(), Some(ed.doc.items[0].id));
        }
    }

    #[test]
    fn a_tiny_drag_creates_no_shape_but_a_click_makes_a_pen_dot() {
        let mut ed = Editor::new(100, 100);
        for tool in [
            Tool::Line,
            Tool::Arrow,
            Tool::Rect,
            Tool::Ellipse,
            Tool::Blur,
            Tool::Pixelate,
        ] {
            ed.tool = tool;
            drag(&mut ed, p(10.0, 10.0), p(11.0, 11.0), NO);
        }
        assert!(ed.doc.items.is_empty());
        ed.tool = Tool::Pen;
        drag(&mut ed, p(10.0, 10.0), p(10.0, 10.0), NO);
        assert_eq!(ed.doc.items.len(), 1);
    }

    #[test]
    fn shift_snaps_lines_and_squares_rectangles() {
        let mut ed = Editor::new(300, 300);
        ed.tool = Tool::Line;
        drag(&mut ed, p(10.0, 10.0), p(110.0, 15.0), SHIFT);
        let Annotation::Line { to, .. } = only(&ed) else {
            panic!()
        };
        assert!(
            (to.y - 10.0).abs() < 1e-3 && (to.x - 110.0).abs() < 1.0,
            "{to:?}"
        );

        let mut ed = Editor::new(300, 300);
        ed.tool = Tool::Ellipse;
        drag(&mut ed, p(10.0, 10.0), p(110.0, 60.0), SHIFT);
        let Annotation::Ellipse { rect, .. } = only(&ed) else {
            panic!()
        };
        assert_eq!((rect.w, rect.h), (100.0, 100.0));
    }

    #[test]
    fn the_pointer_is_clamped_to_the_image() {
        let mut ed = Editor::new(100, 50);
        ed.tool = Tool::Rect;
        drag(&mut ed, p(-20.0, -20.0), p(500.0, 500.0), NO);
        let Annotation::Rect { rect, .. } = only(&ed) else {
            panic!()
        };
        assert_eq!(*rect, Rect::new(0.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn the_preview_follows_the_gesture_and_vanishes_after() {
        let mut ed = Editor::new(100, 100);
        ed.tool = Tool::Rect;
        ed.pointer_down(p(10.0, 10.0), NO, None);
        ed.pointer_move(p(50.0, 40.0), NO);
        let Some(Annotation::Rect { rect, .. }) = ed.preview() else {
            panic!()
        };
        assert_eq!(rect, Rect::new(10.0, 10.0, 40.0, 30.0));
        assert!(ed.doc.items.is_empty());
        ed.pointer_up(p(50.0, 40.0), NO);
        assert!(ed.preview().is_none());
    }

    #[test]
    fn markers_number_themselves() {
        let mut ed = Editor::new(100, 100);
        ed.tool = Tool::Marker;
        for x in [10.0, 30.0, 50.0] {
            ed.pointer_down(p(x, 10.0), NO, None);
            ed.pointer_up(p(x, 10.0), NO);
        }
        let numbers: Vec<u32> = ed
            .doc
            .items
            .iter()
            .filter_map(|i| {
                if let Annotation::Marker { number, .. } = i.annotation {
                    Some(number)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(numbers, [1, 2, 3]);
        // delete the last one: the next click reuses its number
        ed.delete_selected();
        ed.pointer_down(p(70.0, 10.0), NO, None);
        assert_eq!(ed.doc.next_marker_number(), 4);
    }

    #[test]
    fn text_flow() {
        let mut ed = Editor::new(200, 100);
        ed.tool = Tool::Text;
        assert_eq!(
            ed.pointer_down(p(20.0, 20.0), NO, None),
            Outcome::TextRequested(p(20.0, 20.0))
        );
        assert!(ed.add_text(p(20.0, 20.0), "   ").is_none());
        let id = ed.add_text(p(20.0, 20.0), "Hello").unwrap();
        ed.edit_text(id, "Bye");
        let Annotation::Text { text, .. } = only(&ed) else {
            panic!()
        };
        assert_eq!(text, "Bye");
        ed.undo();
        let Annotation::Text { text, .. } = only(&ed) else {
            panic!()
        };
        assert_eq!(text, "Hello");
    }

    #[test]
    fn select_move_delete_undo() {
        let mut ed = Editor::new(200, 100);
        ed.tool = Tool::Rect;
        ed.settings.filled = true;
        drag(&mut ed, p(10.0, 10.0), p(60.0, 60.0), NO);
        ed.tool = Tool::Select;
        ed.pointer_down(p(30.0, 30.0), NO, None);
        assert_eq!(ed.hidden(), Some(ed.doc.items[0].id));
        ed.pointer_move(p(50.0, 40.0), NO);
        let Some(Annotation::Rect { rect, .. }) = ed.preview() else {
            panic!()
        };
        assert_eq!((rect.x, rect.y), (30.0, 20.0));
        ed.pointer_up(p(50.0, 40.0), NO);
        let Annotation::Rect { rect, .. } = only(&ed) else {
            panic!()
        };
        assert_eq!((rect.x, rect.y), (30.0, 20.0));

        ed.nudge(1.0, 1.0);
        ed.undo();
        ed.undo(); // the move
        let Annotation::Rect { rect, .. } = only(&ed) else {
            panic!()
        };
        assert_eq!((rect.x, rect.y), (10.0, 10.0));
        ed.redo();

        ed.delete_selected();
        assert!(ed.doc.items.is_empty());
        assert_eq!(ed.selection(), None);
        ed.undo();
        assert_eq!(ed.doc.items.len(), 1);
    }

    #[test]
    fn clicking_empty_space_deselects() {
        let mut ed = Editor::new(100, 100);
        ed.tool = Tool::Rect;
        drag(&mut ed, p(10.0, 10.0), p(30.0, 30.0), NO);
        assert!(ed.selection().is_some());
        ed.tool = Tool::Select;
        ed.pointer_down(p(90.0, 90.0), NO, None);
        assert!(ed.selection().is_none());
        assert!(ed.hidden().is_none());
    }

    #[test]
    fn a_click_without_movement_does_not_create_an_edit_when_selecting() {
        let mut ed = Editor::new(100, 100);
        ed.tool = Tool::Rect;
        ed.settings.filled = true;
        drag(&mut ed, p(10.0, 10.0), p(60.0, 60.0), NO);
        ed.tool = Tool::Select;
        ed.pointer_down(p(30.0, 30.0), NO, None);
        ed.pointer_up(p(30.0, 30.0), NO);
        ed.undo(); // only the creation is in the history
        assert!(ed.doc.items.is_empty());
    }

    #[test]
    fn crop_tool_sets_and_clears_the_crop() {
        let mut ed = Editor::new(100, 100);
        ed.tool = Tool::Crop;
        ed.pointer_down(p(10.0, 10.0), NO, None);
        ed.pointer_move(p(60.0, 50.0), NO);
        assert_eq!(ed.crop_preview(), Some(Rect::new(10.0, 10.0, 50.0, 40.0)));
        ed.pointer_up(p(60.0, 50.0), NO);
        assert_eq!(ed.doc.crop, Some(Rect::new(10.0, 10.0, 50.0, 40.0)));
        ed.clear_crop();
        assert_eq!(ed.doc.crop, None);
        ed.undo();
        assert!(ed.doc.crop.is_some());
        assert!(ed.doc.items.is_empty());
    }

    #[test]
    fn eyedropper_reads_the_image_and_remembers_colours() {
        let mut ed = Editor::new(4, 4);
        let base = RgbaImage::filled(4, 4, Color::rgb(10, 20, 30));
        ed.tool = Tool::Eyedropper;
        assert_eq!(
            ed.pointer_down(p(1.0, 1.0), NO, Some(&base)),
            Outcome::Picked(Color::rgb(10, 20, 30))
        );
        assert_eq!(ed.settings.style.color, Color::rgb(10, 20, 30));
        assert_eq!(
            ed.pointer_down(p(9.0, 1.0), NO, Some(&base)),
            Outcome::Nothing
        );
        assert_eq!(ed.recent_colors(), [Color::rgb(10, 20, 30)]);
        assert!(ed.doc.items.is_empty());
    }

    #[test]
    fn recent_colours_are_unique_and_bounded() {
        let mut ed = Editor::new(1, 1);
        for i in 0..12u8 {
            ed.set_color(Color::rgb(i, 0, 0));
        }
        ed.set_color(Color::rgb(5, 0, 0));
        assert_eq!(ed.recent_colors().len(), 8);
        assert_eq!(ed.recent_colors()[0], Color::rgb(5, 0, 0));
        assert_eq!(
            ed.recent_colors()
                .iter()
                .filter(|c| **c == Color::rgb(5, 0, 0))
                .count(),
            1
        );
    }

    #[test]
    fn undo_during_a_gesture_cancels_it() {
        let mut ed = Editor::new(100, 100);
        ed.tool = Tool::Rect;
        ed.pointer_down(p(10.0, 10.0), NO, None);
        ed.undo();
        ed.pointer_up(p(50.0, 50.0), NO);
        assert!(ed.doc.items.is_empty());
    }
}
