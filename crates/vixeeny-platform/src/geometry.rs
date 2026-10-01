// SPDX-License-Identifier: GPL-3.0-or-later
//! Pixel rectangles on the virtual desktop. Origin may be negative (monitors left of or above
//! the primary one).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Exclusive right edge.
    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// Exclusive bottom edge.
    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        i64::from(x) >= i64::from(self.x)
            && i64::from(x) < self.right()
            && i64::from(y) >= i64::from(self.y)
            && i64::from(y) < self.bottom()
    }

    /// Overlap, `None` when the rectangles do not share a pixel.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let left = i64::from(self.x).max(i64::from(other.x));
        let top = i64::from(self.y).max(i64::from(other.y));
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        Some(Self::new(
            i32::try_from(left).ok()?,
            i32::try_from(top).ok()?,
            u32::try_from(right - left).ok()?,
            u32::try_from(bottom - top).ok()?,
        ))
    }

    /// Smallest rectangle containing both.
    pub fn union(&self, other: &Self) -> Self {
        let left = i64::from(self.x).min(i64::from(other.x));
        let top = i64::from(self.y).min(i64::from(other.y));
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::new(
            clamp_i32(left),
            clamp_i32(top),
            u32::try_from(right - left).unwrap_or(u32::MAX),
            u32::try_from(bottom - top).unwrap_or(u32::MAX),
        )
    }

    /// Squared distance from the point to the rectangle (0 inside).
    pub fn distance2(&self, x: i32, y: i32) -> u64 {
        let dx = (i64::from(self.x) - i64::from(x))
            .max(i64::from(x) - (self.right() - 1))
            .max(0);
        let dy = (i64::from(self.y) - i64::from(y))
            .max(i64::from(y) - (self.bottom() - 1))
            .max(0);
        (dx * dx + dy * dy) as u64
    }

    /// The same rectangle expressed relative to `origin`'s top-left corner.
    pub fn relative_to(&self, origin: &Self) -> Self {
        Self::new(
            clamp_i32(i64::from(self.x) - i64::from(origin.x)),
            clamp_i32(i64::from(self.y) - i64::from(origin.y)),
            self.width,
            self.height,
        )
    }
}

fn clamp_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

/// Bounding box of all rectangles (the virtual desktop), `None` for an empty list.
pub fn virtual_bounds<'a>(
    rects: impl IntoIterator<Item = &'a PhysicalRect>,
) -> Option<PhysicalRect> {
    rects.into_iter().copied().reduce(|a, b| a.union(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_and_union() {
        let a = PhysicalRect::new(0, 0, 100, 100);
        let b = PhysicalRect::new(50, 60, 100, 100);
        assert_eq!(a.intersection(&b), Some(PhysicalRect::new(50, 60, 50, 40)));
        assert_eq!(a.union(&b), PhysicalRect::new(0, 0, 150, 160));
        assert_eq!(a.intersection(&PhysicalRect::new(100, 0, 10, 10)), None);
    }

    #[test]
    fn negative_origins() {
        // A monitor to the left of the primary one starts at a negative x.
        let left = PhysicalRect::new(-1920, 0, 1920, 1080);
        let main = PhysicalRect::new(0, 0, 2560, 1440);
        let all = virtual_bounds([&left, &main]).unwrap();
        assert_eq!(all, PhysicalRect::new(-1920, 0, 4480, 1440));
        assert!(left.contains(-1, 0));
        assert!(!left.contains(0, 0));
        assert_eq!(
            main.relative_to(&all),
            PhysicalRect::new(1920, 0, 2560, 1440)
        );
    }

    #[test]
    fn distance() {
        let r = PhysicalRect::new(10, 10, 10, 10);
        assert_eq!(r.distance2(15, 15), 0);
        assert_eq!(r.distance2(0, 15), 100);
        assert_eq!(r.distance2(22, 22), 18); // 3² + 3²: the last pixel is 19
    }

    #[test]
    fn empty_and_none() {
        assert!(PhysicalRect::new(0, 0, 0, 5).is_empty());
        assert_eq!(virtual_bounds([]), None);
    }
}
