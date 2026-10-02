// SPDX-License-Identifier: GPL-3.0-or-later
//! Points and rectangles in image pixels (origin top-left, y down).

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    pub fn offset(self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
}

/// Axis-aligned rectangle with non-negative size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The rectangle spanned by two opposite corners, in any order.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (a.x - b.x).abs(),
            (a.y - b.y).abs(),
        )
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn inflate(&self, by: f32) -> Self {
        Self::new(
            self.x - by,
            self.y - by,
            self.w + 2.0 * by,
            self.h + 2.0 * by,
        )
    }

    pub fn translate(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn union(&self, other: &Self) -> Self {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        Self::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let (r, b) = (
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        );
        (r > x && b > y).then(|| Self::new(x, y, r - x, b - y))
    }

    /// Bounding box of points; `None` when empty.
    pub fn of_points(points: &[Point]) -> Option<Self> {
        let first = points.first()?;
        let mut r = Self::new(first.x, first.y, 0.0, 0.0);
        for p in &points[1..] {
            r = r.union(&Self::new(p.x, p.y, 0.0, 0.0));
        }
        Some(r)
    }
}

/// Distance from `p` to the segment `a`–`b`.
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

/// Snaps the direction `from`→`to` to the nearest multiple of `step_deg`, keeping the length
/// (Shift while drawing a line or an arrow).
pub fn snap_angle(from: Point, to: Point, step_deg: f32) -> Point {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = dx.hypot(dy);
    if len == 0.0 {
        return to;
    }
    let step = step_deg.to_radians();
    let angle = (dy.atan2(dx) / step).round() * step;
    Point::new(from.x + len * angle.cos(), from.y + len * angle.sin())
}

/// Moves `to` so that the rectangle `from`–`to` is a square (Shift on rectangle/ellipse).
pub fn square_corner(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let side = dx.abs().max(dy.abs());
    Point::new(
        from.x + side.copysign(if dx == 0.0 { 1.0 } else { dx }),
        from.y + side.copysign(if dy == 0.0 { 1.0 } else { dy }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_in_any_order() {
        let r = Rect::from_corners(Point::new(10.0, 20.0), Point::new(4.0, 5.0));
        assert_eq!(r, Rect::new(4.0, 5.0, 6.0, 15.0));
    }

    #[test]
    fn intersection_and_union() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.intersect(&Rect::new(20.0, 0.0, 1.0, 1.0)), None);
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
    }

    #[test]
    fn segment_distance() {
        let (a, b) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        assert_eq!(distance_to_segment(Point::new(5.0, 3.0), a, b), 3.0);
        assert_eq!(distance_to_segment(Point::new(-3.0, 4.0), a, b), 5.0);
        assert_eq!(distance_to_segment(Point::new(1.0, 1.0), a, a), 2f32.sqrt());
    }

    #[test]
    fn angle_snapping() {
        let from = Point::new(0.0, 0.0);
        // ~10° snaps to 15°
        let to = Point::new(
            100.0 * 10f32.to_radians().cos(),
            100.0 * 10f32.to_radians().sin(),
        );
        let s = snap_angle(from, to, 15.0);
        let angle = s.y.atan2(s.x).to_degrees();
        assert!((angle - 15.0).abs() < 1e-3, "{angle}");
        assert!((from.distance(s) - 100.0).abs() < 1e-3);
        // ~2° snaps to the horizontal
        let s = snap_angle(from, Point::new(100.0, 3.0), 15.0);
        assert!(s.y.abs() < 1e-3);
        assert_eq!(snap_angle(from, from, 15.0), from);
    }

    #[test]
    fn square_follows_the_longer_side_and_the_direction() {
        let from = Point::new(10.0, 10.0);
        assert_eq!(
            square_corner(from, Point::new(30.0, 15.0)),
            Point::new(30.0, 30.0)
        );
        assert_eq!(
            square_corner(from, Point::new(12.0, -10.0)),
            Point::new(30.0, -10.0)
        );
    }
}
