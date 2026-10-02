// SPDX-License-Identifier: GPL-3.0-or-later
//! Flattens a base image and its annotations into the exported pixels.

use tiny_skia::{
    BlendMode, FillRule, IntSize, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke,
    Transform,
};

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

impl RgbaImage {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (data.len() == width as usize * height as usize * 4).then_some(Self {
            width,
            height,
            data,
        })
    }

    pub fn filled(width: u32, height: u32, c: Color) -> Self {
        let data = [c.r, c.g, c.b, c.a].repeat(width as usize * height as usize);
        Self {
            width,
            height,
            data,
        }
    }

    /// From BGRA rows `stride` bytes apart (what the capture crate produces). Alpha is forced
    /// to opaque: screen captures have none.
    pub fn from_bgra(width: u32, height: u32, stride: usize, bgra: &[u8]) -> Option<Self> {
        let row = width as usize * 4;
        if stride < row || bgra.len() < stride * height as usize {
            return None;
        }
        let mut data = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            for px in bgra[y * stride..y * stride + row].as_chunks::<4>().0.iter() {
                data.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        Some(Self {
            width,
            height,
            data,
        })
    }

    pub fn pixel(&self, x: u32, y: u32) -> Color {
        let o = (y as usize * self.width as usize + x as usize) * 4;
        Color {
            r: self.data[o],
            g: self.data[o + 1],
            b: self.data[o + 2],
            a: self.data[o + 3],
        }
    }

    pub(crate) fn crop(&self, region: Region) -> Self {
        let mut data = Vec::with_capacity(region.w * region.h * 4);
        for y in region.y..region.y + region.h {
            let o = (y * self.width as usize + region.x) * 4;
            data.extend_from_slice(&self.data[o..o + region.w * 4]);
        }
        Self {
            width: region.w as u32,
            height: region.h as u32,
            data,
        }
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
    Stroke {
        width: width.max(0.5),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// Quadratic smoothing through the midpoints of consecutive points.
fn smooth_path(points: &[Point]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let first = points.first()?;
    pb.move_to(first.x, first.y);
    match points.len() {
        1 => pb.line_to(first.x + 0.01, first.y),
        2 => pb.line_to(points[1].x, points[1].y),
        n => {
            for i in 1..n - 1 {
                let (c, next) = (points[i], points[i + 1]);
                pb.quad_to(c.x, c.y, (c.x + next.x) / 2.0, (c.y + next.y) / 2.0);
            }
            let last = points[n - 1];
            pb.line_to(last.x, last.y);
        }
    }
    pb.finish()
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
    let font = text::font();
    let (w, h) = (pix.width() as i32, pix.height() as i32);
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
                let (x, y) = (gx + col as i32, gy + row as i32);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let o = (y as usize * w as usize + x as usize) * 4;
                blend_pixel(
                    &mut pix.data_mut()[o..o + 4],
                    color,
                    coverage[row * m.width + col],
                );
            }
        }
    }
}

fn draw_annotation(pix: &mut Pixmap, a: &Annotation) {
    let id = Transform::identity();
    match a {
        Annotation::Pen { points, style } => {
            if let Some(path) = smooth_path(points) {
                pix.stroke_path(
                    &path,
                    &paint(style.color, BlendMode::SourceOver),
                    &stroke(style.width),
                    id,
                    None,
                );
            }
        }
        Annotation::Highlight {
            points,
            color,
            width,
        } => {
            if let Some(path) = smooth_path(points) {
                pix.stroke_path(
                    &path,
                    &paint(*color, BlendMode::Multiply),
                    &stroke(*width),
                    id,
                    None,
                );
            }
        }
        Annotation::Line { from, to, style } => {
            if let Some(path) = smooth_path(&[*from, *to]) {
                pix.stroke_path(
                    &path,
                    &paint(style.color, BlendMode::SourceOver),
                    &stroke(style.width),
                    id,
                    None,
                );
            }
        }
        Annotation::Arrow { from, to, style } => {
            draw_arrow(pix, *from, *to, style.color, style.width)
        }
        Annotation::Rect {
            rect,
            style,
            filled,
        } => {
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
        Annotation::Ellipse {
            rect,
            style,
            filled,
        } => {
            if let Some(path) = skia_rect(rect).and_then(PathBuilder::from_oval) {
                let p = paint(style.color, BlendMode::SourceOver);
                if *filled {
                    pix.fill_path(&path, &p, FillRule::Winding, id, None);
                } else {
                    pix.stroke_path(&path, &p, &stroke(style.width), id, None);
                }
            }
        }
        Annotation::Text {
            pos,
            text,
            size,
            color,
            background,
        } => {
            if let Some(bg) = background {
                let b = a.bounds().inflate(size * 0.2);
                if let Some(r) = skia_rect(&b) {
                    pix.fill_path(
                        &PathBuilder::from_rect(r),
                        &paint(*bg, BlendMode::SourceOver),
                        FillRule::Winding,
                        id,
                        None,
                    );
                }
            }
            draw_text(pix, *pos, text, *size, *color);
        }
        Annotation::Marker {
            center,
            number,
            color,
            radius,
        } => {
            if let Some(path) = PathBuilder::from_circle(center.x, center.y, *radius) {
                pix.fill_path(
                    &path,
                    &paint(*color, BlendMode::SourceOver),
                    FillRule::Winding,
                    id,
                    None,
                );
            }
            let size = radius * 1.2;
            let label = number.to_string();
            let (_, w, h) = text::layout(&label, size);
            let ink = if luminance(*color) > 140.0 {
                Color::rgb(0, 0, 0)
            } else {
                Color::rgb(255, 255, 255)
            };
            draw_text(
                pix,
                Point::new(center.x - w / 2.0, center.y - h / 2.0),
                &label,
                size,
                ink,
            );
        }
        Annotation::Blur { .. } | Annotation::Pixelate { .. } => {} // pixel effects, see `render`
    }
}

fn draw_arrow(pix: &mut Pixmap, from: Point, to: Point, color: Color, width: f32) {
    let len = from.distance(to);
    if len < 0.5 {
        return;
    }
    let (ux, uy) = ((to.x - from.x) / len, (to.y - from.y) / len);
    let head = arrow_head(width).min(len);
    let base = Point::new(to.x - ux * head * 0.8, to.y - uy * head * 0.8);
    let p = paint(color, BlendMode::SourceOver);
    if let Some(path) = smooth_path(&[from, base]) {
        pix.stroke_path(&path, &p, &stroke(width), Transform::identity(), None);
    }
    let half = head * 0.45;
    let mut pb = PathBuilder::new();
    pb.move_to(to.x, to.y);
    pb.line_to(to.x - ux * head - uy * half, to.y - uy * head + ux * half);
    pb.line_to(to.x - ux * head + uy * half, to.y - uy * head - ux * half);
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
    // The base is opaque, so straight and premultiplied RGBA are the same bytes.
    let Some(mut pix) = Pixmap::from_vec(image.data.clone(), size) else {
        return image;
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
    RgbaImage {
        width,
        height,
        data: pix.take(),
    }
}

/// The part of `base` inside `region` (rounded outwards, clamped) with `annotations` drawn on
/// it. `None` when the region is empty. Only the region is rasterised, so a live preview of a
/// small selection stays cheap on a large screen.
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

/// Renders the document over `base`: annotations bottom to top, then the crop.
pub fn render(base: &RgbaImage, doc: &Document) -> RgbaImage {
    render_region(
        base,
        doc.items.iter().map(|i| &i.annotation),
        &doc.output_rect(),
    )
    .unwrap_or_else(|| base.clone())
}

#[cfg(test)]
mod tests;
