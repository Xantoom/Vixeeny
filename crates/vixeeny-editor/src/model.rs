// SPDX-License-Identifier: GPL-3.0-or-later
//! Annotations and the document that holds them.

use crate::geometry::{Point, Rect, distance_to_segment};
use crate::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// `#rrggbb` (the eyedropper and the colour field).
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.is_ascii() {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Self::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    /// The eight preset colours of the palette (plan 5.3).
    pub const PALETTE: [Self; 8] = [
        Self::rgb(0xE5, 0x39, 0x35), // red
        Self::rgb(0xFB, 0x8C, 0x00), // orange
        Self::rgb(0xFD, 0xD8, 0x35), // yellow
        Self::rgb(0x43, 0xA0, 0x47), // green
        Self::rgb(0x1E, 0x88, 0xE5), // blue
        Self::rgb(0x8E, 0x24, 0xAA), // purple
        Self::rgb(0xFF, 0xFF, 0xFF), // white
        Self::rgb(0x21, 0x21, 0x21), // black
    ];
}

/// Colour and thickness of a stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub color: Color,
    pub width: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: Color::PALETTE[0],
            width: 4.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Annotation {
    /// Smoothed freehand stroke.
    Pen { points: Vec<Point>, style: Style },
    Line {
        from: Point,
        to: Point,
        style: Style,
    },
    Arrow {
        from: Point,
        to: Point,
        style: Style,
    },
    Rect {
        rect: Rect,
        style: Style,
        filled: bool,
    },
    Ellipse {
        rect: Rect,
        style: Style,
        filled: bool,
    },
    Text {
        pos: Point,
        text: String,
        size: f32,
        color: Color,
        background: Option<Color>,
    },
    /// Wide semi-transparent stroke drawn in multiply mode.
    Highlight {
        points: Vec<Point>,
        color: Color,
        width: f32,
    },
    /// Gaussian blur of the pixels below, `radius` in pixels. Irreversible once exported.
    Blur { rect: Rect, radius: f32 },
    /// Mosaic of `block`×`block` pixels. Irreversible once exported.
    Pixelate { rect: Rect, block: u32 },
    /// Numbered badge.
    Marker {
        center: Point,
        number: u32,
        color: Color,
        radius: f32,
    },
}

/// Distance beyond which a click no longer selects an annotation.
pub const HIT_TOLERANCE: f32 = 4.0;

fn polyline_distance(p: Point, points: &[Point]) -> f32 {
    match points {
        [] => f32::INFINITY,
        [only] => p.distance(*only),
        _ => points
            .windows(2)
            .map(|w| distance_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

fn rect_outline_distance(p: Point, r: &Rect) -> f32 {
    let corners = [
        Point::new(r.x, r.y),
        Point::new(r.right(), r.y),
        Point::new(r.right(), r.bottom()),
        Point::new(r.x, r.bottom()),
    ];
    (0..4)
        .map(|i| distance_to_segment(p, corners[i], corners[(i + 1) % 4]))
        .fold(f32::INFINITY, f32::min)
}

impl Annotation {
    /// Smallest rectangle containing the drawing (stroke thickness included).
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Pen { points, style } => Rect::of_points(points)
                .unwrap_or_default()
                .inflate(style.width / 2.0),
            Self::Highlight { points, width, .. } => Rect::of_points(points)
                .unwrap_or_default()
                .inflate(width / 2.0),
            Self::Line { from, to, style } => {
                Rect::from_corners(*from, *to).inflate(style.width / 2.0)
            }
            Self::Arrow { from, to, style } => {
                Rect::from_corners(*from, *to).inflate(style.width / 2.0 + arrow_head(style.width))
            }
            Self::Rect { rect, style, .. } | Self::Ellipse { rect, style, .. } => {
                rect.inflate(style.width / 2.0)
            }
            Self::Text {
                pos, text, size, ..
            } => {
                let (_, w, h) = text::layout(text, *size);
                Rect::new(pos.x, pos.y, w, h)
            }
            Self::Blur { rect, .. } | Self::Pixelate { rect, .. } => *rect,
            Self::Marker { center, radius, .. } => Rect::new(
                center.x - radius,
                center.y - radius,
                2.0 * radius,
                2.0 * radius,
            ),
        }
    }

    /// Whether a click at `p` grabs this annotation.
    pub fn hit(&self, p: Point) -> bool {
        match self {
            Self::Pen { points, style } => {
                polyline_distance(p, points) <= style.width / 2.0 + HIT_TOLERANCE
            }
            Self::Highlight { points, width, .. } => {
                polyline_distance(p, points) <= width / 2.0 + HIT_TOLERANCE
            }
            Self::Line { from, to, style } | Self::Arrow { from, to, style } => {
                distance_to_segment(p, *from, *to) <= style.width / 2.0 + HIT_TOLERANCE
            }
            Self::Rect {
                rect,
                style,
                filled,
            } => {
                if *filled {
                    rect.inflate(HIT_TOLERANCE).contains(p)
                } else {
                    rect_outline_distance(p, rect) <= style.width / 2.0 + HIT_TOLERANCE
                }
            }
            Self::Ellipse {
                rect,
                style,
                filled,
            } => {
                let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                let tol = style.width / 2.0 + HIT_TOLERANCE;
                let (rx, ry) = ((rect.w / 2.0).max(0.5), (rect.h / 2.0).max(0.5));
                // Normalised radius: 1.0 on the outline.
                let d = ((p.x - cx) / rx).hypot((p.y - cy) / ry);
                let scale = rx.min(ry);
                if *filled {
                    (d - 1.0) * scale <= tol
                } else {
                    ((d - 1.0) * scale).abs() <= tol
                }
            }
            Self::Text { .. } | Self::Blur { .. } | Self::Pixelate { .. } => {
                self.bounds().contains(p)
            }
            Self::Marker { center, radius, .. } => p.distance(*center) <= radius + HIT_TOLERANCE,
        }
    }

    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        let mv = |p: &Point| p.offset(dx, dy);
        match self {
            Self::Pen { points, style } => Self::Pen {
                points: points.iter().map(mv).collect(),
                style: *style,
            },
            Self::Highlight {
                points,
                color,
                width,
            } => Self::Highlight {
                points: points.iter().map(mv).collect(),
                color: *color,
                width: *width,
            },
            Self::Line { from, to, style } => Self::Line {
                from: mv(from),
                to: mv(to),
                style: *style,
            },
            Self::Arrow { from, to, style } => Self::Arrow {
                from: mv(from),
                to: mv(to),
                style: *style,
            },
            Self::Rect {
                rect,
                style,
                filled,
            } => Self::Rect {
                rect: rect.translate(dx, dy),
                style: *style,
                filled: *filled,
            },
            Self::Ellipse {
                rect,
                style,
                filled,
            } => Self::Ellipse {
                rect: rect.translate(dx, dy),
                style: *style,
                filled: *filled,
            },
            Self::Text {
                pos,
                text,
                size,
                color,
                background,
            } => Self::Text {
                pos: mv(pos),
                text: text.clone(),
                size: *size,
                color: *color,
                background: *background,
            },
            Self::Blur { rect, radius } => Self::Blur {
                rect: rect.translate(dx, dy),
                radius: *radius,
            },
            Self::Pixelate { rect, block } => Self::Pixelate {
                rect: rect.translate(dx, dy),
                block: *block,
            },
            Self::Marker {
                center,
                number,
                color,
                radius,
            } => Self::Marker {
                center: mv(center),
                number: *number,
                color: *color,
                radius: *radius,
            },
        }
    }
}

/// Length of the arrow head for a given stroke width.
pub fn arrow_head(width: f32) -> f32 {
    6.0 + 3.0 * width
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnnotationId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: AnnotationId,
    pub annotation: Annotation,
}

/// The annotations over an image of `size` pixels, bottom to top, and the final crop.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub size: (u32, u32),
    pub items: Vec<Item>,
    pub crop: Option<Rect>,
    next_id: u32,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width, height),
            items: Vec::new(),
            crop: None,
            next_id: 1,
        }
    }

    pub fn fresh_id(&mut self) -> AnnotationId {
        let id = AnnotationId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| &i.annotation)
    }

    /// Topmost annotation under `p`.
    pub fn hit_test(&self, p: Point) -> Option<AnnotationId> {
        self.items
            .iter()
            .rev()
            .find(|i| i.annotation.hit(p))
            .map(|i| i.id)
    }

    /// Number of the next marker: one more than the highest present.
    pub fn next_marker_number(&self) -> u32 {
        self.items
            .iter()
            .filter_map(|i| match i.annotation {
                Annotation::Marker { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .map_or(1, |n| n + 1)
    }

    /// The area that ends up in the exported image: the crop, clamped to the image.
    pub fn output_rect(&self) -> Rect {
        let full = Rect::new(0.0, 0.0, self.size.0 as f32, self.size.1 as f32);
        self.crop.and_then(|c| c.intersect(&full)).unwrap_or(full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(w: f32) -> Style {
        Style {
            color: Color::rgb(1, 2, 3),
            width: w,
        }
    }

    #[test]
    fn hex_colours() {
        assert_eq!(Color::rgb(0xE5, 0x39, 0x35).hex(), "#e53935");
        assert_eq!(
            Color::from_hex("#1e88e5"),
            Some(Color::rgb(0x1E, 0x88, 0xE5))
        );
        assert_eq!(
            Color::from_hex("1e88e5"),
            Some(Color::rgb(0x1E, 0x88, 0xE5))
        );
        assert_eq!(Color::from_hex("#12"), None);
        assert_eq!(Color::from_hex("#gggggg"), None);
        assert_eq!(Color::from_hex("#é1234"), None);
    }

    #[test]
    fn line_hit_and_bounds() {
        let a = Annotation::Line {
            from: Point::new(0.0, 0.0),
            to: Point::new(100.0, 0.0),
            style: style(4.0),
        };
        assert!(a.hit(Point::new(50.0, 5.0)));
        assert!(!a.hit(Point::new(50.0, 20.0)));
        assert_eq!(a.bounds(), Rect::new(-2.0, -2.0, 104.0, 4.0));
    }

    #[test]
    fn outline_rect_is_hollow_but_filled_rect_is_not() {
        let r = Rect::new(10.0, 10.0, 100.0, 100.0);
        let outline = Annotation::Rect {
            rect: r,
            style: style(2.0),
            filled: false,
        };
        let filled = Annotation::Rect {
            rect: r,
            style: style(2.0),
            filled: true,
        };
        let inside = Point::new(60.0, 60.0);
        assert!(!outline.hit(inside));
        assert!(filled.hit(inside));
        assert!(outline.hit(Point::new(10.0, 60.0)));
    }

    #[test]
    fn ellipse_hit() {
        let e = Annotation::Ellipse {
            rect: Rect::new(0.0, 0.0, 100.0, 50.0),
            style: style(2.0),
            filled: false,
        };
        assert!(e.hit(Point::new(100.0, 25.0)));
        assert!(!e.hit(Point::new(50.0, 25.0)));
        let f = Annotation::Ellipse {
            rect: Rect::new(0.0, 0.0, 100.0, 50.0),
            style: style(2.0),
            filled: true,
        };
        assert!(f.hit(Point::new(50.0, 25.0)));
        assert!(!f.hit(Point::new(2.0, 2.0)));
    }

    #[test]
    fn translation_moves_everything() {
        let a = Annotation::Pen {
            points: vec![Point::new(1.0, 1.0), Point::new(2.0, 3.0)],
            style: style(1.0),
        };
        let b = a.translated(10.0, -1.0);
        assert_eq!(
            b,
            Annotation::Pen {
                points: vec![Point::new(11.0, 0.0), Point::new(12.0, 2.0)],
                style: style(1.0)
            }
        );
        assert_eq!(b.translated(-10.0, 1.0), a);
    }

    #[test]
    fn markers_count_up_from_the_highest() {
        let mut doc = Document::new(10, 10);
        assert_eq!(doc.next_marker_number(), 1);
        for n in [1, 3] {
            let id = doc.fresh_id();
            doc.items.push(Item {
                id,
                annotation: Annotation::Marker {
                    center: Point::default(),
                    number: n,
                    color: Color::PALETTE[0],
                    radius: 10.0,
                },
            });
        }
        assert_eq!(doc.next_marker_number(), 4);
    }

    #[test]
    fn output_rect_is_clamped_to_the_image() {
        let mut doc = Document::new(100, 50);
        assert_eq!(doc.output_rect(), Rect::new(0.0, 0.0, 100.0, 50.0));
        doc.crop = Some(Rect::new(-10.0, 10.0, 60.0, 100.0));
        assert_eq!(doc.output_rect(), Rect::new(0.0, 10.0, 50.0, 40.0));
        doc.crop = Some(Rect::new(500.0, 500.0, 5.0, 5.0));
        assert_eq!(doc.output_rect(), Rect::new(0.0, 0.0, 100.0, 50.0));
    }
}
