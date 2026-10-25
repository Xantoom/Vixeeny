// SPDX-License-Identifier: GPL-3.0-or-later
//! Flattens a base image and its annotations into the exported pixels.

use tiny_skia::{BlendMode, FillRule, IntSize, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::effects::{self, Region};
use crate::geometry::{Point, Rect};
use crate::model::{Annotation, Color, Document, arrow_head};
use crate::text;

/// Straight (non-premultiplied) RGBA, 8 bits per channel, row-major without padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Converts `rows` rows of BGRA pixels (`src_stride` bytes apart) to opaque RGBA rows (`dst_stride` apart), `width`
/// pixels each. Several threads share the rows: two 4K screens are 66 MB.
pub fn bgra_to_rgba(src: &[u8], src_stride: usize, dst: &mut [u8], dst_stride: usize, (width, rows): (usize, usize)) {
    if rows == 0 || width == 0 {
        return;
    }
    let dst_len = (rows - 1) * dst_stride + width * 4;
    let Some(dst) = dst.get_mut(..dst_len) else {
        return;
    };
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get).min(8).min(rows.max(1));
    let per = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        for (i, chunk) in dst.chunks_mut(per * dst_stride).enumerate() {
            let first = i * per;
            scope.spawn(move || {
                for (y, out) in chunk.chunks_mut(dst_stride).enumerate() {
                    let at = (first + y) * src_stride;
                    let Some(input) = src.get(at..at + width * 4) else {
                        return;
                    };
                    let out = &mut out[..width * 4];
                    let pixels = out.as_chunks_mut::<4>().0.iter_mut().zip(input.as_chunks::<4>().0);
                    for (o, i) in pixels {
                        *o = [i[2], i[1], i[0], 255];
                    }
                }
            });
        }
    });
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (data.len() == width as usize * height as usize * 4).then_some(Self { width, height, data })
    }

    pub fn filled(width: u32, height: u32, c: Color) -> Self {
        let data = [c.r, c.g, c.b, c.a].repeat(width as usize * height as usize);
        Self { width, height, data }
    }

    /// From BGRA rows `stride` bytes apart (what the capture crate produces). Alpha is forced to opaque: screen
    /// captures have none.
    pub fn from_bgra(width: u32, height: u32, stride: usize, bgra: &[u8]) -> Option<Self> {
        let row = width as usize * 4;
        if stride < row || bgra.len() < stride * height as usize {
            return None;
        }
        let mut data = vec![0; row * height as usize];
        bgra_to_rgba(bgra, stride, &mut data, row, (width as usize, height as usize));
        Some(Self { width, height, data })
    }

    pub fn pixel(&self, x: u32, y: u32) -> Color {
        let o = (y as usize * self.width as usize + x as usize) * 4;
        Color { r: self.data[o], g: self.data[o + 1], b: self.data[o + 2], a: self.data[o + 3] }
    }

    pub(crate) fn crop(&self, region: Region) -> Self {
        let mut data = Vec::with_capacity(region.w * region.h * 4);
        for y in region.y..region.y + region.h {
            let o = (y * self.width as usize + region.x) * 4;
            data.extend_from_slice(&self.data[o..o + region.w * 4]);
        }
        Self { width: region.w as u32, height: region.h as u32, data }
    }
}

fn paint(c: Color, blend: BlendMode) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.r, c.g, c.b, c.a);
    p.anti_alias = true;
    p.blend_mode = blend;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke { width: stroke_width(width), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

/// A piece of a stroke's path, after its first point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Line(Point),
    /// Control point, end point.
    Quad(Point, Point),
}

/// The path of a stroke through `points`: quadratic smoothing through the midpoints of consecutive points. Its first
/// point, then its segments; `None` without points.
pub fn smooth(points: &[Point]) -> Option<(Point, Vec<Segment>)> {
    let first = *points.first()?;
    let segments = match points.len() {
        1 => vec![Segment::Line(Point::new(first.x + 0.01, first.y))],
        2 => vec![Segment::Line(points[1])],
        n => {
            let mut s: Vec<Segment> = (1..n - 1)
                .map(|i| {
                    let (c, next) = (points[i], points[i + 1]);
                    Segment::Quad(c, Point::new((c.x + next.x) / 2.0, (c.y + next.y) / 2.0))
                })
                .collect();
            s.push(Segment::Line(points[n - 1]));
            s
        }
    };
    Some((first, segments))
}

fn smooth_path(points: &[Point]) -> Option<Path> {
    let (first, segments) = smooth(points)?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    for s in segments {
        match s {
            Segment::Line(p) => pb.line_to(p.x, p.y),
            Segment::Quad(c, p) => pb.quad_to(c.x, c.y, p.x, p.y),
        }
    }
    pb.finish()
}

/// The thickness a stroke of `width` is drawn with.
pub fn stroke_width(width: f32) -> f32 {
    width.max(0.5)
}

/// An arrow: its shaft ends at `shaft_end`, its head is a triangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowShape {
    pub shaft_end: Point,
    pub head: [Point; 3],
}

/// The arrow from `from` to `to`, `width` thick; `None` when it has no length.
pub fn arrow_shape(from: Point, to: Point, width: f32) -> Option<ArrowShape> {
    let len = from.distance(to);
    if len < 0.5 {
        return None;
    }
    let (ux, uy) = ((to.x - from.x) / len, (to.y - from.y) / len);
    let head = arrow_head(width).min(len);
    let half = head * 0.45;
    Some(ArrowShape {
        shaft_end: Point::new(to.x - ux * head * 0.8, to.y - uy * head * 0.8),
        head: [
            to,
            Point::new(to.x - ux * head - uy * half, to.y - uy * head + ux * half),
            Point::new(to.x - ux * head + uy * half, to.y - uy * head - ux * half),
        ],
    })
}

/// The box behind a text that has a background.
pub fn text_background(a: &Annotation) -> Option<(Rect, Color)> {
    match a {
        Annotation::Text { size, background: Some(bg), .. } => Some((a.bounds().inflate(size * 0.2), *bg)),
        _ => None,
    }
}

/// The colour of a marker's number: dark on a light badge, light on a dark one.
pub fn marker_ink(color: Color) -> Color {
    if luminance(color) > 140.0 { Color::rgb(0, 0, 0) } else { Color::rgb(255, 255, 255) }
}

/// What a text or a marker's number is written with: the text, its top-left corner, its size and colour.
fn writing(a: &Annotation) -> Option<(String, Point, f32, Color)> {
    match a {
        Annotation::Text { pos, text, size, color, .. } => Some((text.clone(), *pos, *size, *color)),
        Annotation::Marker { center, number, color, radius } => {
            let size = radius * 1.2;
            let label = number.to_string();
            let (_, w, h) = text::layout(&label, size);
            Some((label, Point::new(center.x - w / 2.0, center.y - h / 2.0), size, marker_ink(*color)))
        }
        _ => None,
    }
}

/// Visits each glyph pixel of a writing: `(x, y, coverage)`, image pixels.
fn glyph_pixels(origin: Point, content: &str, size: f32, mut f: impl FnMut(i32, i32, u8)) {
    let font = text::font();
    let (glyphs, ..) = text::layout(content, size);
    for g in glyphs {
        let (m, coverage) = font.rasterize(g.ch, size);
        if m.width == 0 {
            continue;
        }
        let gx = (origin.x + g.x).round() as i32 + m.xmin;
        let gy = (origin.y + g.baseline).round() as i32 - m.ymin - m.height as i32;
        for row in 0..m.height {
            for col in 0..m.width {
                f(gx + col as i32, gy + row as i32, coverage[row * m.width + col]);
            }
        }
    }
}

/// The letters of a text or of a marker's number, drawn alone (straight alpha), with where the image's top-left corner
/// goes; `None` for other annotations or no letters.
pub fn letters(a: &Annotation) -> Option<(RgbaImage, i32, i32)> {
    let (content, origin, size, color) = writing(a)?;
    let mut hits = Vec::new();
    glyph_pixels(origin, &content, size, |x, y, c| hits.push((x, y, c)));
    let x0 = hits.iter().map(|h| h.0).min()?;
    let y0 = hits.iter().map(|h| h.1).min()?;
    let w = (hits.iter().map(|h| h.0).max()? - x0 + 1) as u32;
    let h = (hits.iter().map(|h| h.1).max()? - y0 + 1) as u32;
    let mut image = RgbaImage { width: w, height: h, data: vec![0; w as usize * h as usize * 4] };
    for (x, y, c) in hits {
        let o = ((y - y0) as usize * w as usize + (x - x0) as usize) * 4;
        let px = &mut image.data[o..o + 4];
        // Overlapping glyphs: the most covered wins.
        let a = (u32::from(c) * u32::from(color.a) / 255) as u8;
        if a >= px[3] {
            px.copy_from_slice(&[color.r, color.g, color.b, a]);
        }
    }
    Some((image, x0, y0))
}

fn skia_rect(r: &Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_xywh(r.x, r.y, r.w.max(0.01), r.h.max(0.01))
}

fn luminance(c: Color) -> f32 {
    0.2126 * f32::from(c.r) + 0.7152 * f32::from(c.g) + 0.0722 * f32::from(c.b)
}

/// Source-over of `color` with coverage `cov` onto a premultiplied pixel.
fn blend_pixel(dst: &mut [u8], color: Color, cov: u8) {
    let a = u32::from(cov) * u32::from(color.a) / 255;
    let inv = 255 - a;
    for (d, s) in dst.iter_mut().zip([color.r, color.g, color.b]) {
        *d = ((u32::from(s) * a + u32::from(*d) * inv + 127) / 255) as u8;
    }
    dst[3] = (a + u32::from(dst[3]) * inv / 255).min(255) as u8;
}

fn draw_text(pix: &mut Pixmap, origin: Point, content: &str, size: f32, color: Color) {
    let (w, h) = (pix.width() as i32, pix.height() as i32);
    glyph_pixels(origin, content, size, |x, y, coverage| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        let o = (y as usize * w as usize + x as usize) * 4;
        blend_pixel(&mut pix.data_mut()[o..o + 4], color, coverage);
    });
}

fn draw_annotation(pix: &mut Pixmap, a: &Annotation) {
    let id = Transform::identity();
    match a {
        Annotation::Pen { points, style } => {
            if let Some(path) = smooth_path(points) {
                pix.stroke_path(&path, &paint(style.color, BlendMode::SourceOver), &stroke(style.width), id, None);
            }
        }
        Annotation::Highlight { points, color, width } => {
            if let Some(path) = smooth_path(points) {
                pix.stroke_path(&path, &paint(*color, BlendMode::Multiply), &stroke(*width), id, None);
            }
        }
        Annotation::Line { from, to, style } => {
            if let Some(path) = smooth_path(&[*from, *to]) {
                pix.stroke_path(&path, &paint(style.color, BlendMode::SourceOver), &stroke(style.width), id, None);
            }
        }
        Annotation::Arrow { from, to, style } => draw_arrow(pix, *from, *to, style.color, style.width),
        Annotation::Rect { rect, style, filled } => {
            if let Some(r) = skia_rect(rect) {
                let path = PathBuilder::from_rect(r);
                let p = paint(style.color, BlendMode::SourceOver);
                if *filled {
                    pix.fill_path(&path, &p, FillRule::Winding, id, None);
                } else {
                    pix.stroke_path(&path, &p, &stroke(style.width), id, None);
                }
            }
        }
        Annotation::Ellipse { rect, style, filled } => {
            if let Some(path) = skia_rect(rect).and_then(PathBuilder::from_oval) {
                let p = paint(style.color, BlendMode::SourceOver);
                if *filled {
                    pix.fill_path(&path, &p, FillRule::Winding, id, None);
                } else {
                    pix.stroke_path(&path, &p, &stroke(style.width), id, None);
                }
            }
        }
        Annotation::Text { .. } => {
            if let Some((b, bg)) = text_background(a)
                && let Some(r) = skia_rect(&b)
            {
                pix.fill_path(
                    &PathBuilder::from_rect(r),
                    &paint(bg, BlendMode::SourceOver),
                    FillRule::Winding,
                    id,
                    None,
                );
            }
            if let Some((content, origin, size, color)) = writing(a) {
                draw_text(pix, origin, &content, size, color);
            }
        }
        Annotation::Marker { center, color, radius, .. } => {
            if let Some(path) = PathBuilder::from_circle(center.x, center.y, *radius) {
                pix.fill_path(&path, &paint(*color, BlendMode::SourceOver), FillRule::Winding, id, None);
            }
            if let Some((content, origin, size, color)) = writing(a) {
                draw_text(pix, origin, &content, size, color);
            }
        }
        Annotation::Blur { .. } | Annotation::Pixelate { .. } => {} // pixel effects, see `render`
    }
}

fn draw_arrow(pix: &mut Pixmap, from: Point, to: Point, color: Color, width: f32) {
    let Some(shape) = arrow_shape(from, to, width) else {
        return;
    };
    let p = paint(color, BlendMode::SourceOver);
    if let Some(path) = smooth_path(&[from, shape.shaft_end]) {
        pix.stroke_path(&path, &p, &stroke(width), Transform::identity(), None);
    }
    let [a, b, c] = shape.head;
    let mut pb = PathBuilder::new();
    pb.move_to(a.x, a.y);
    pb.line_to(b.x, b.y);
    pb.line_to(c.x, c.y);
    pb.close();
    if let Some(path) = pb.finish() {
        pix.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
}

/// Draws `annotations` (in the coordinates of `image`) over `image`, bottom to top.
fn flatten<'a>(image: RgbaImage, annotations: impl Iterator<Item = &'a Annotation>) -> RgbaImage {
    let Some(size) = IntSize::from_wh(image.width, image.height) else {
        return image;
    };
    let (width, height) = (image.width, image.height);
    if image.data.len() != width as usize * height as usize * 4 {
        return image;
    }
    // The base is opaque, so straight and premultiplied RGBA are the same bytes. Its pixels become the drawing's: no
    // copy (two 4K screens are 66 MB).
    let Some(mut pix) = Pixmap::from_vec(image.data, size) else {
        // Not reached: the length was checked above.
        return RgbaImage::filled(width, height, Color::rgb(0, 0, 0));
    };
    let (w, h) = (width as usize, height as usize);
    for annotation in annotations {
        match annotation {
            Annotation::Blur { rect, radius } => {
                if let Some(region) = Region::clamp(rect, w, h) {
                    effects::blur(pix.data_mut(), w, region, *radius);
                }
            }
            Annotation::Pixelate { rect, block } => {
                if let Some(region) = Region::clamp(rect, w, h) {
                    effects::pixelate(pix.data_mut(), w, region, *block as usize);
                }
            }
            other => draw_annotation(&mut pix, other),
        }
    }
    RgbaImage { width, height, data: pix.take() }
}

/// The part of `base` inside `region` (rounded outwards, clamped) with `annotations` drawn on it. `None` when the
/// region is empty. Only the region is rasterised, so a live preview of a small selection stays cheap on a large
/// screen.
pub fn render_region<'a>(
    base: &RgbaImage,
    annotations: impl Iterator<Item = &'a Annotation>,
    region: &Rect,
) -> Option<RgbaImage> {
    let r = Region::clamp(region, base.width as usize, base.height as usize)?;
    let (dx, dy) = (-(r.x as f32), -(r.y as f32));
    let moved: Vec<Annotation> = annotations.map(|a| a.translated(dx, dy)).collect();
    Some(flatten(base.crop(r), moved.iter()))
}

/// Renders the document over `base`: annotations bottom to top.
pub fn render(base: &RgbaImage, doc: &Document) -> RgbaImage {
    flatten(base.clone(), doc.items.iter().map(|i| &i.annotation))
}

#[cfg(test)]
mod tests;
