// SPDX-License-Identifier: GPL-3.0-or-later
//! The capture area (plan 5.3 steps 3–5): drawing a zone, resizing and moving it with handles,
//! window detection, and where the toolbar, size label and magnifier go. Pure geometry; all
//! coordinates are pixels of the frozen image.

use crate::geometry::{Point, Rect};

/// Smallest side of a selection, in pixels.
pub const MIN_SIDE: f32 = 4.0;
/// A press-release that moved less than this is a click (selects the window under the cursor).
pub const CLICK_SLOP: f32 = 3.0;
/// Half-size of a handle's hit area, in pixels.
pub const HANDLE_RADIUS: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    pub const ALL: [Self; 8] = [
        Self::NW,
        Self::N,
        Self::NE,
        Self::E,
        Self::SE,
        Self::S,
        Self::SW,
        Self::W,
    ];

    pub fn position(self, r: &Rect) -> Point {
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        match self {
            Self::N => Point::new(cx, r.y),
            Self::S => Point::new(cx, r.bottom()),
            Self::E => Point::new(r.right(), cy),
            Self::W => Point::new(r.x, cy),
            Self::NE => Point::new(r.right(), r.y),
            Self::NW => Point::new(r.x, r.y),
            Self::SE => Point::new(r.right(), r.bottom()),
            Self::SW => Point::new(r.x, r.bottom()),
        }
    }
}

/// Mouse cursor to show, for the UI to map to the platform's cursors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorHint {
    Crosshair,
    Move,
    ResizeNs,
    ResizeEw,
    ResizeNeSw,
    ResizeNwSe,
    Default,
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Idle,
    /// Drawing a new zone from `start`.
    Drawing {
        start: Point,
    },
    Moving {
        grab: Point,
        original: Rect,
    },
    Resizing {
        handle: Handle,
        original: Rect,
    },
}

#[derive(Debug, Clone)]
pub struct Selection {
    bounds: Rect,
    rect: Option<Rect>,
    mode: Mode,
    /// Window rectangles, topmost first, in image pixels.
    windows: Vec<Rect>,
    press: Option<Point>,
    pub hover_window: Option<Rect>,
    /// Where the zone was just drawn (the pointer still rests on its corner): there the zone
    /// moves rather than resizes, until the pointer goes away.
    fresh: Option<Point>,
}

impl Selection {
    pub fn new(bounds: Rect, windows: Vec<Rect>) -> Self {
        Self {
            bounds,
            rect: None,
            mode: Mode::Idle,
            windows,
            press: None,
            hover_window: None,
            fresh: None,
        }
    }

    /// The zone, once one exists (also while it is being drawn).
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    pub fn is_dragging(&self) -> bool {
        !matches!(self.mode, Mode::Idle)
    }

    /// The zone is being drawn: the magnifier helps start it on the right pixel. Not while it
    /// is resized or moved (the zone itself is then what the eye follows).
    pub fn is_drawing(&self) -> bool {
        matches!(self.mode, Mode::Drawing { .. })
    }

    /// The pointer still rests where the zone was just drawn.
    fn rests_on_fresh_zone(&self, p: Point) -> bool {
        self.fresh
            .is_some_and(|at| at.distance(p) <= HANDLE_RADIUS * 2.0)
    }

    /// A zone exists and no gesture is running: the toolbar can show.
    pub fn is_settled(&self) -> bool {
        self.rect.is_some() && !self.is_dragging()
    }

    fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.bounds.x, self.bounds.right()),
            p.y.clamp(self.bounds.y, self.bounds.bottom()),
        )
    }

    fn window_at(&self, p: Point) -> Option<Rect> {
        self.windows
            .iter()
            .find(|w| w.contains(p))
            .and_then(|w| w.intersect(&self.bounds))
    }

    /// Which handle (if any) is under `p`.
    pub fn handle_at(&self, p: Point) -> Option<Handle> {
        let r = self.rect?;
        Handle::ALL
            .into_iter()
            .find(|h| h.position(&r).distance(p) <= HANDLE_RADIUS * 1.5)
            .or_else(|| {
                // Edges between the handles: grabbing the border resizes too.
                let e = HANDLE_RADIUS;
                let inner = r.inflate(-e).contains(p);
                let outer = r.inflate(e).contains(p);
                if inner || !outer {
                    return None;
                }
                let (near_l, near_r) = ((p.x - r.x).abs() <= e, (p.x - r.right()).abs() <= e);
                let (near_t, near_b) = ((p.y - r.y).abs() <= e, (p.y - r.bottom()).abs() <= e);
                match (near_l, near_r, near_t, near_b) {
                    (true, _, true, _) => Some(Handle::NW),
                    (_, true, true, _) => Some(Handle::NE),
                    (true, _, _, true) => Some(Handle::SW),
                    (_, true, _, true) => Some(Handle::SE),
                    (true, ..) => Some(Handle::W),
                    (_, true, ..) => Some(Handle::E),
                    (_, _, true, _) => Some(Handle::N),
                    (.., true) => Some(Handle::S),
                    _ => None,
                }
            })
    }

    pub fn cursor_at(&self, p: Point) -> CursorHint {
        if self.rect.is_some() && self.rests_on_fresh_zone(p) {
            return CursorHint::Move;
        }
        if let Some(h) = self.handle_at(p) {
            return match h {
                Handle::N | Handle::S => CursorHint::ResizeNs,
                Handle::E | Handle::W => CursorHint::ResizeEw,
                Handle::NE | Handle::SW => CursorHint::ResizeNeSw,
                Handle::NW | Handle::SE => CursorHint::ResizeNwSe,
            };
        }
        match self.rect {
            Some(r) if r.contains(p) => CursorHint::Move,
            _ => CursorHint::Crosshair,
        }
    }

    pub fn pointer_down(&mut self, p: Point) {
        let p = self.clamp(p);
        self.press = Some(p);
        let fresh = self.rests_on_fresh_zone(p);
        self.fresh = None;
        self.mode = if let (true, Some(original)) = (fresh, self.rect) {
            Mode::Moving { grab: p, original }
        } else if let (Some(handle), Some(original)) = (self.handle_at(p), self.rect) {
            Mode::Resizing { handle, original }
        } else if let Some(original) = self.rect.filter(|r| r.contains(p)) {
            Mode::Moving { grab: p, original }
        } else {
            self.rect = None;
            Mode::Drawing { start: p }
        };
    }

    pub fn pointer_move(&mut self, p: Point) {
        let p = self.clamp(p);
        match self.mode {
            Mode::Idle => {
                if !self.rests_on_fresh_zone(p) {
                    self.fresh = None;
                }
                self.hover_window = if self.rect.is_none() {
                    self.window_at(p)
                } else {
                    None
                }
            }
            Mode::Drawing { start } => self.rect = Some(Rect::from_corners(start, p)),
            Mode::Moving { grab, original } => {
                let (dx, dy) = (p.x - grab.x, p.y - grab.y);
                let x = (original.x + dx).clamp(self.bounds.x, self.bounds.right() - original.w);
                let y = (original.y + dy).clamp(self.bounds.y, self.bounds.bottom() - original.h);
                self.rect = Some(Rect::new(x, y, original.w, original.h));
            }
            Mode::Resizing { handle, original } => self.rect = Some(resize(&original, handle, p)),
        }
    }

    pub fn pointer_up(&mut self, p: Point) {
        self.pointer_move(p);
        let p = self.clamp(p);
        let was_click = self
            .press
            .is_some_and(|press| press.distance(p) < CLICK_SLOP);
        if matches!(self.mode, Mode::Drawing { .. }) {
            self.rect = if was_click {
                // A plain click takes the whole window under the cursor.
                self.window_at(p)
            } else {
                self.rect.filter(|r| r.w >= MIN_SIDE && r.h >= MIN_SIDE)
            };
            self.fresh = self.rect.map(|_| p);
        } else if let Some(r) = self.rect {
            self.rect = Some(self.keep_min(r));
        }
        self.mode = Mode::Idle;
        self.press = None;
        self.hover_window = None;
    }

    fn keep_min(&self, r: Rect) -> Rect {
        Rect::new(r.x, r.y, r.w.max(MIN_SIDE), r.h.max(MIN_SIDE))
    }

    /// Forgets the zone (to start over).
    pub fn reset(&mut self) {
        self.rect = None;
        self.fresh = None;
        self.mode = Mode::Idle;
    }

    /// Takes the whole frozen image as the zone (Ctrl+A style shortcut).
    pub fn select_all(&mut self) {
        self.rect = Some(self.bounds);
    }
}

fn resize(original: &Rect, handle: Handle, p: Point) -> Rect {
    let (mut l, mut t, mut r, mut b) =
        (original.x, original.y, original.right(), original.bottom());
    match handle {
        Handle::N => t = p.y,
        Handle::S => b = p.y,
        Handle::E => r = p.x,
        Handle::W => l = p.x,
        Handle::NE => (t, r) = (p.y, p.x),
        Handle::NW => (t, l) = (p.y, p.x),
        Handle::SE => (b, r) = (p.y, p.x),
        Handle::SW => (b, l) = (p.y, p.x),
    }
    // Dragging an edge past the opposite one flips the rectangle instead of inverting it.
    Rect::from_corners(Point::new(l, t), Point::new(r, b))
}

/// Where to put a box of `size` next to the zone: below it, else above, else inside at the
/// bottom — kept within `bounds` (plan 5.3 step 4). `gap` separates box and zone.
pub fn place_beside(zone: &Rect, size: (f32, f32), bounds: &Rect, gap: f32) -> Point {
    let x = (zone.right() - size.0).clamp(bounds.x, (bounds.right() - size.0).max(bounds.x));
    let below = zone.bottom() + gap;
    let above = zone.y - gap - size.1;
    let y = if below + size.1 <= bounds.bottom() {
        below
    } else if above >= bounds.y {
        above
    } else {
        (zone.bottom() - size.1 - gap).max(bounds.y)
    };
    Point::new(x, y)
}

/// The square of source pixels shown by the magnifier: `side` pixels centred on the cursor,
/// shifted to stay inside the image (the edge pixel is then magnified off-centre).
pub fn magnifier_source(cursor: Point, side: u32, bounds: &Rect) -> Rect {
    let side = side as f32;
    let x = (cursor.x.floor() - (side / 2.0).floor())
        .clamp(bounds.x, (bounds.right() - side).max(bounds.x));
    let y = (cursor.y.floor() - (side / 2.0).floor())
        .clamp(bounds.y, (bounds.bottom() - side).max(bounds.y));
    Rect::new(x, y, side, side)
}

/// Where the magnifier box goes: down-right of the cursor, flipping to the other side near the
/// edges so it never covers the pixel being inspected.
pub fn magnifier_position(cursor: Point, size: (f32, f32), bounds: &Rect, offset: f32) -> Point {
    let x = if cursor.x + offset + size.0 <= bounds.right() {
        cursor.x + offset
    } else {
        cursor.x - offset - size.0
    };
    let y = if cursor.y + offset + size.1 <= bounds.bottom() {
        cursor.y + offset
    } else {
        cursor.y - offset - size.1
    };
    Point::new(x.max(bounds.x), y.max(bounds.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }

    fn sel() -> Selection {
        Selection::new(
            Rect::new(0.0, 0.0, 1000.0, 600.0),
            vec![
                Rect::new(100.0, 100.0, 300.0, 200.0),
                Rect::new(0.0, 0.0, 1000.0, 600.0),
            ],
        )
    }

    fn draw(s: &mut Selection, a: Point, b: Point) {
        s.pointer_down(a);
        s.pointer_move(p((a.x + b.x) / 2.0, (a.y + b.y) / 2.0));
        s.pointer_up(b);
    }

    #[test]
    fn drawing_a_zone() {
        let mut s = sel();
        s.pointer_down(p(50.0, 60.0));
        s.pointer_move(p(150.0, 160.0));
        assert!(s.is_dragging());
        assert_eq!(s.rect(), Some(Rect::new(50.0, 60.0, 100.0, 100.0)));
        s.pointer_up(p(150.0, 160.0));
        assert!(s.is_settled());
        // drawing backwards works too, and stays inside the image
        let mut s = sel();
        draw(&mut s, p(500.0, 500.0), p(2000.0, -50.0));
        assert_eq!(s.rect(), Some(Rect::new(500.0, 0.0, 500.0, 500.0)));
    }

    #[test]
    fn a_click_selects_the_topmost_window_under_the_cursor() {
        let mut s = sel();
        s.pointer_move(p(150.0, 150.0));
        assert_eq!(s.hover_window, Some(Rect::new(100.0, 100.0, 300.0, 200.0)));
        s.pointer_down(p(150.0, 150.0));
        s.pointer_up(p(151.0, 150.0));
        assert_eq!(s.rect(), Some(Rect::new(100.0, 100.0, 300.0, 200.0)));
        let mut s = sel();
        s.pointer_move(p(800.0, 500.0));
        assert_eq!(s.hover_window, Some(Rect::new(0.0, 0.0, 1000.0, 600.0)));
    }

    #[test]
    fn a_click_with_no_window_selects_nothing() {
        let mut s = Selection::new(Rect::new(0.0, 0.0, 100.0, 100.0), vec![]);
        s.pointer_down(p(10.0, 10.0));
        s.pointer_up(p(10.0, 10.0));
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn windows_are_clipped_to_the_image() {
        let mut s = Selection::new(
            Rect::new(0.0, 0.0, 100.0, 100.0),
            vec![Rect::new(50.0, 50.0, 200.0, 200.0)],
        );
        s.pointer_down(p(60.0, 60.0));
        s.pointer_up(p(60.0, 60.0));
        assert_eq!(s.rect(), Some(Rect::new(50.0, 50.0, 50.0, 50.0)));
    }

    #[test]
    fn too_small_a_drag_is_dropped() {
        let mut s = sel();
        draw(&mut s, p(10.0, 10.0), p(12.0, 40.0));
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn moving_keeps_the_size_and_stays_inside() {
        let mut s = sel();
        draw(&mut s, p(100.0, 100.0), p(200.0, 150.0));
        s.pointer_down(p(150.0, 120.0));
        s.pointer_move(p(250.0, 140.0));
        assert_eq!(s.rect(), Some(Rect::new(200.0, 120.0, 100.0, 50.0)));
        s.pointer_move(p(5000.0, 5000.0));
        s.pointer_up(p(5000.0, 5000.0));
        assert_eq!(s.rect(), Some(Rect::new(900.0, 550.0, 100.0, 50.0)));
    }

    #[test]
    fn every_handle_resizes_its_side() {
        let base = Rect::new(100.0, 100.0, 200.0, 100.0);
        let cases = [
            (
                Handle::N,
                p(200.0, 80.0),
                Rect::new(100.0, 80.0, 200.0, 120.0),
            ),
            (
                Handle::S,
                p(200.0, 260.0),
                Rect::new(100.0, 100.0, 200.0, 160.0),
            ),
            (
                Handle::E,
                p(350.0, 150.0),
                Rect::new(100.0, 100.0, 250.0, 100.0),
            ),
            (
                Handle::W,
                p(50.0, 150.0),
                Rect::new(50.0, 100.0, 250.0, 100.0),
            ),
            (
                Handle::NE,
                p(320.0, 90.0),
                Rect::new(100.0, 90.0, 220.0, 110.0),
            ),
            (
                Handle::NW,
                p(90.0, 90.0),
                Rect::new(90.0, 90.0, 210.0, 110.0),
            ),
            (
                Handle::SE,
                p(320.0, 210.0),
                Rect::new(100.0, 100.0, 220.0, 110.0),
            ),
            (
                Handle::SW,
                p(90.0, 210.0),
                Rect::new(90.0, 100.0, 210.0, 110.0),
            ),
        ];
        for (handle, to, expected) in cases {
            let mut s = sel();
            draw(&mut s, p(100.0, 100.0), p(300.0, 200.0));
            assert_eq!(s.rect(), Some(base));
            s.pointer_move(p(200.0, 150.0)); // away from where the zone was drawn
            let start = handle.position(&base);
            assert_eq!(s.handle_at(start), Some(handle), "{handle:?}");
            s.pointer_down(start);
            s.pointer_move(to);
            s.pointer_up(to);
            assert_eq!(s.rect(), Some(expected), "{handle:?}");
        }
    }

    #[test]
    fn dragging_an_edge_past_the_other_flips_the_zone() {
        let mut s = sel();
        draw(&mut s, p(100.0, 100.0), p(300.0, 200.0));
        s.pointer_down(p(300.0, 150.0)); // E handle
        s.pointer_move(p(40.0, 150.0));
        s.pointer_up(p(40.0, 150.0));
        assert_eq!(s.rect(), Some(Rect::new(40.0, 100.0, 60.0, 100.0)));
    }

    #[test]
    fn the_border_between_handles_resizes() {
        let mut s = sel();
        draw(&mut s, p(100.0, 100.0), p(300.0, 300.0));
        assert_eq!(s.handle_at(p(150.0, 101.0)), Some(Handle::N));
        assert_eq!(s.handle_at(p(299.0, 200.0)), Some(Handle::E));
        assert_eq!(s.handle_at(p(200.0, 200.0)), None);
        assert_eq!(s.handle_at(p(600.0, 600.0)), None);
    }

    #[test]
    fn cursor_hints() {
        let mut s = sel();
        assert_eq!(s.cursor_at(p(5.0, 5.0)), CursorHint::Crosshair);
        draw(&mut s, p(100.0, 100.0), p(300.0, 300.0));
        assert_eq!(s.cursor_at(p(200.0, 200.0)), CursorHint::Move);
        assert_eq!(s.cursor_at(p(100.0, 100.0)), CursorHint::ResizeNwSe);
        assert_eq!(s.cursor_at(p(300.0, 100.0)), CursorHint::ResizeNeSw);
        assert_eq!(s.cursor_at(p(200.0, 100.0)), CursorHint::ResizeNs);
        assert_eq!(s.cursor_at(p(100.0, 200.0)), CursorHint::ResizeEw);
        assert_eq!(s.cursor_at(p(500.0, 500.0)), CursorHint::Crosshair);
    }

    #[test]
    fn a_zone_just_drawn_moves_from_where_the_pointer_rests() {
        let mut s = sel();
        draw(&mut s, p(100.0, 100.0), p(300.0, 300.0));
        // The pointer is on the bottom-right handle, yet the zone moves from there.
        assert_eq!(s.cursor_at(p(300.0, 300.0)), CursorHint::Move);
        assert!(!s.is_drawing());
        s.pointer_down(p(300.0, 300.0));
        s.pointer_move(p(350.0, 320.0));
        assert!(s.is_dragging() && !s.is_drawing());
        s.pointer_up(p(350.0, 320.0));
        assert_eq!(s.rect(), Some(Rect::new(150.0, 120.0, 200.0, 200.0)));
        // Once the pointer has gone away, the handle resizes again.
        assert_eq!(s.cursor_at(p(350.0, 320.0)), CursorHint::ResizeNwSe);
        s.pointer_down(p(350.0, 320.0));
        assert_eq!(s.rect(), Some(Rect::new(150.0, 120.0, 200.0, 200.0)));
        s.pointer_move(p(400.0, 400.0));
        // Resized from the handle (no magnifier then).
        assert_eq!(s.rect(), Some(Rect::new(150.0, 120.0, 250.0, 280.0)));
        assert!(!s.is_drawing());
    }

    #[test]
    fn leaving_the_corner_of_a_fresh_zone_brings_the_handle_back() {
        let mut s = sel();
        draw(&mut s, p(100.0, 100.0), p(300.0, 300.0));
        s.pointer_move(p(200.0, 200.0));
        s.pointer_move(p(300.0, 300.0));
        assert_eq!(s.cursor_at(p(300.0, 300.0)), CursorHint::ResizeNwSe);
    }

    #[test]
    fn pressing_outside_starts_a_new_zone() {
        let mut s = sel();
        draw(&mut s, p(100.0, 100.0), p(200.0, 200.0));
        draw(&mut s, p(500.0, 300.0), p(600.0, 400.0));
        assert_eq!(s.rect(), Some(Rect::new(500.0, 300.0, 100.0, 100.0)));
    }

    #[test]
    fn select_all_and_reset() {
        let mut s = sel();
        s.select_all();
        assert_eq!(s.rect(), Some(Rect::new(0.0, 0.0, 1000.0, 600.0)));
        s.reset();
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn toolbar_goes_below_then_above_then_inside() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 600.0);
        let size = (300.0, 40.0);
        // plenty of room below
        let zone = Rect::new(100.0, 100.0, 400.0, 200.0);
        assert_eq!(place_beside(&zone, size, &bounds, 8.0), p(200.0, 308.0));
        // zone touches the bottom: above
        let zone = Rect::new(100.0, 300.0, 400.0, 300.0);
        assert_eq!(place_beside(&zone, size, &bounds, 8.0), p(200.0, 252.0));
        // zone is the whole screen: inside, at the bottom
        let zone = bounds;
        assert_eq!(place_beside(&zone, size, &bounds, 8.0), p(700.0, 552.0));
        // right-aligned but never off-screen on the left
        let zone = Rect::new(0.0, 100.0, 100.0, 50.0);
        assert_eq!(place_beside(&zone, size, &bounds, 8.0).x, 0.0);
    }

    #[test]
    fn magnifier_source_stays_in_the_image() {
        let b = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(
            magnifier_source(p(50.5, 25.5), 11, &b),
            Rect::new(45.0, 20.0, 11.0, 11.0)
        );
        assert_eq!(
            magnifier_source(p(0.0, 0.0), 11, &b),
            Rect::new(0.0, 0.0, 11.0, 11.0)
        );
        assert_eq!(
            magnifier_source(p(99.9, 49.9), 11, &b),
            Rect::new(89.0, 39.0, 11.0, 11.0)
        );
    }

    #[test]
    fn magnifier_flips_near_the_edges() {
        let b = Rect::new(0.0, 0.0, 1000.0, 600.0);
        let size = (120.0, 140.0);
        assert_eq!(
            magnifier_position(p(100.0, 100.0), size, &b, 20.0),
            p(120.0, 120.0)
        );
        assert_eq!(
            magnifier_position(p(950.0, 100.0), size, &b, 20.0),
            p(810.0, 120.0)
        );
        assert_eq!(
            magnifier_position(p(100.0, 580.0), size, &b, 20.0),
            p(120.0, 420.0)
        );
    }
}
