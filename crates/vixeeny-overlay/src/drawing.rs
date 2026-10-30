// SPDX-License-Identifier: GPL-3.0-or-later
//! The annotations drawn by the graphics card, over a picture of the screens under them (frozen, or the newest live
//! one), with the shapes of the export (`vixeeny_editor::render`).
//!
//! Everything is drawn into one bitmap the size of the area the annotations cover: the screens, then each annotation in
//! order. A blur, a pixelation or a highlight works on what is under it, so the bitmap is copied aside first and the
//! effect drawn back from the copy. Live, a second bitmap tells where something is drawn: only there does the result
//! cover the screens, and the live pictures of the screens are copied in on the graphics card.

use vixeeny_editor::render::{Segment, arrow_shape, letters, smooth, stroke_width, text_background};
use vixeeny_editor::{Annotation, Color, Point, Rect, RgbaImage};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BLEND_MODE_MULTIPLY, D2D1_BORDER_MODE_HARD,
    D2D1_COLOR_F, D2D1_COMPOSITE_MODE, D2D1_COMPOSITE_MODE_DESTINATION_IN, D2D1_COMPOSITE_MODE_SOURCE_COPY,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1Blend, CLSID_D2D1Border, CLSID_D2D1ColorMatrix, CLSID_D2D1Crop, CLSID_D2D1GaussianBlur, CLSID_D2D1Scale,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_BLEND_PROP_MODE,
    D2D1_BORDER_EDGE_MODE_CLAMP, D2D1_BORDER_PROP_EDGE_MODE_X, D2D1_BORDER_PROP_EDGE_MODE_Y, D2D1_CAP_STYLE_ROUND,
    D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT, D2D1_COLORMATRIX_PROP_COLOR_MATRIX, D2D1_CROP_PROP_RECT, D2D1_DASH_STYLE_SOLID,
    D2D1_ELLIPSE, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_LINE_JOIN_ROUND,
    D2D1_PROPERTY_TYPE_BOOL, D2D1_PROPERTY_TYPE_ENUM, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_MATRIX_5X4,
    D2D1_PROPERTY_TYPE_VECTOR2, D2D1_PROPERTY_TYPE_VECTOR4, D2D1_QUADRATIC_BEZIER_SEGMENT,
    D2D1_SCALE_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_SCALE_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_SCALE_PROP_BORDER_MODE, D2D1_SCALE_PROP_CENTER_POINT, D2D1_SCALE_PROP_INTERPOLATION_MODE,
    D2D1_SCALE_PROP_SCALE, D2D1_STROKE_STYLE_PROPERTIES1, D2D1_STROKE_TRANSFORM_TYPE_NORMAL, ID2D1Bitmap1,
    ID2D1DeviceContext, ID2D1Effect, ID2D1Image, ID2D1PathGeometry, ID2D1StrokeStyle1,
};
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows::core::{Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::gfx::{Canvas, Gfx};

/// A fresh picture of a part of the screens, on the graphics card: `width`×`height` pixels of `texture` (BGRA, on the
/// editor's device) from `from`, to go at `x`, `y` (whole-image pixels).
pub struct ScreenPart {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub texture: ID3D11Texture2D,
    pub from: (u32, u32),
    /// Keeps the picture from being written over while it is used.
    pub keep: Box<dyn std::any::Any>,
}

impl std::fmt::Debug for ScreenPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenPart")
            .field("at", &(self.x, self.y, self.width, self.height))
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

/// The letters of a text or of a marker's number, as a bitmap, and where it goes from the whole pixels of the
/// annotation's position (see [`letters_key`]).
struct Letters {
    bitmap: ID2D1Bitmap1,
    at: D2D_RECT_F,
}

/// What a text's or a marker's letters look like, and the whole pixels the annotation is moved by from it: moved by
/// whole pixels, the letters are the same bitmap. `None` for the others.
fn letters_key(a: &Annotation) -> Option<(Annotation, (f32, f32))> {
    let anchor = match a {
        Annotation::Text { pos, .. } => *pos,
        Annotation::Marker { center, .. } => *center,
        _ => return None,
    };
    let shift = (anchor.x.floor(), anchor.y.floor());
    Some((a.translated(-shift.0, -shift.1), shift))
}

/// What the drawing was last made from: the layers, the picture of the screens, live or not.
type Made = (usize, u64, u64, bool);
type Layers = std::rc::Rc<Vec<Annotation>>;

pub struct Drawing {
    /// Whole-image pixels: x, y, width, height.
    pub area: (u32, u32, u32, u32),
    /// The screens under the area (a target: live pictures are copied in).
    base: ID2D1Bitmap1,
    /// The picture of the screens `base` comes from (see `View::base_version`).
    pub base_version: u64,
    /// Fresh pictures of the screens taken since.
    feeds: u64,
    /// The result: the screens and the annotations.
    result: ID2D1Bitmap1,
    /// Where something is drawn (live).
    mask: Option<ID2D1Bitmap1>,
    /// The result copied aside, for the effects, and a bitmap to draw a highlight in.
    spare: Option<ID2D1Bitmap1>,
    scratch: Option<ID2D1Bitmap1>,
    letters: Vec<(Annotation, Option<Letters>)>,
    stroke: ID2D1StrokeStyle1,
    made: Option<Made>,
    /// Area-local annotations, retained across live screen feeds; the source Rc also keeps cache identity valid.
    translated: Option<(Layers, Layers)>,
    /// Changes each time the result is drawn again, and differs from every other drawing's: a drawing made anew (the
    /// area moved or grew) is never taken for the one before.
    pub version: u64,
}

/// The next [`Drawing::version`].
static NEXT_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_version() -> u64 {
    NEXT_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn colour(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: f32::from(c.a) / 255.0,
    }
}

fn rect_f(r: &Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w.max(0.01), bottom: r.y + r.h.max(0.01) }
}

/// `r` rounded outwards and clamped to `w`×`h` (what the export's effects cover).
fn effect_rect(r: &Rect, w: u32, h: u32) -> Option<D2D_RECT_F> {
    let x0 = r.x.floor().max(0.0);
    let y0 = r.y.floor().max(0.0);
    let x1 = r.right().ceil().min(w as f32);
    let y1 = r.bottom().ceil().min(h as f32);
    (x1 > x0 && y1 > y0).then_some(D2D_RECT_F { left: x0, top: y0, right: x1, bottom: y1 })
}

pub(crate) fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn target(gfx: &Gfx, w: u32, h: u32) -> Result<ID2D1Bitmap1> {
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
        colorContext: std::mem::ManuallyDrop::new(None),
    };
    // SAFETY: plain creation call.
    unsafe { gfx.dc.CreateBitmap(D2D_SIZE_U { width: w, height: h }, None, 0, &props) }
}

/// Draws into `bitmap` with `f`, between `BeginDraw` and `EndDraw` (always called).
pub(crate) fn draw_into(dc: &ID2D1DeviceContext, bitmap: &ID2D1Bitmap1, f: impl FnOnce() -> Result<()>) -> Result<()> {
    // SAFETY: plain state calls; `EndDraw` matches `BeginDraw` whatever `f` returns, and the target is unset after.
    unsafe {
        dc.SetTarget(bitmap);
        dc.SetDpi(96.0, 96.0);
        dc.BeginDraw();
        dc.SetTransform(&Matrix3x2::identity());
        dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let drawn = f();
        let ended = dc.EndDraw(None, None);
        dc.SetTarget(None::<&ID2D1Image>);
        drawn.and(ended)
    }
}

pub(crate) fn effect(dc: &ID2D1DeviceContext, id: &windows::core::GUID, input: &ID2D1Image) -> Result<ID2D1Effect> {
    // SAFETY: plain creation and property calls.
    unsafe {
        let e = dc.CreateEffect(id)?;
        e.SetInput(0, input, true);
        Ok(e)
    }
}

pub(crate) fn set(
    e: &ID2D1Effect,
    prop: i32,
    kind: windows::Win32::Graphics::Direct2D::D2D1_PROPERTY_TYPE,
    data: &[u8],
) -> Result<()> {
    // SAFETY: `data` is the value of the property, of its type.
    unsafe { e.SetValue(prop as u32, kind, data) }
}

pub(crate) fn set_enum(e: &ID2D1Effect, prop: i32, value: i32) -> Result<()> {
    set(e, prop, D2D1_PROPERTY_TYPE_ENUM, &(value as u32).to_le_bytes())
}

pub(crate) fn output(e: &ID2D1Effect) -> Result<ID2D1Image> {
    // SAFETY: plain call.
    unsafe { e.GetOutput() }
}

/// `input` within `r`, its edges repeated outwards (the export's effects only see their area).
pub(crate) fn clamped(dc: &ID2D1DeviceContext, input: &ID2D1Image, r: D2D_RECT_F) -> Result<ID2D1Image> {
    let crop = effect(dc, &CLSID_D2D1Crop, input)?;
    set(&crop, D2D1_CROP_PROP_RECT.0, D2D1_PROPERTY_TYPE_VECTOR4, &floats(&[r.left, r.top, r.right, r.bottom]))?;
    let border = effect(dc, &CLSID_D2D1Border, &output(&crop)?)?;
    set_enum(&border, D2D1_BORDER_PROP_EDGE_MODE_X.0, D2D1_BORDER_EDGE_MODE_CLAMP.0)?;
    set_enum(&border, D2D1_BORDER_PROP_EDGE_MODE_Y.0, D2D1_BORDER_EDGE_MODE_CLAMP.0)?;
    output(&border)
}

pub(crate) fn blurred(dc: &ID2D1DeviceContext, input: &ID2D1Image, r: D2D_RECT_F, sigma: f32) -> Result<ID2D1Image> {
    let blur = effect(dc, &CLSID_D2D1GaussianBlur, &clamped(dc, input, r)?)?;
    set(&blur, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0, D2D1_PROPERTY_TYPE_FLOAT, &sigma.to_le_bytes())?;
    set_enum(&blur, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0, D2D1_BORDER_MODE_HARD.0)?;
    output(&blur)
}

/// Cells of `block` pixels aligned to the area's corner, each of one colour.
fn pixelated(dc: &ID2D1DeviceContext, input: &ID2D1Image, r: D2D_RECT_F, block: u32) -> Result<ID2D1Image> {
    let b = block.max(1) as f32;
    let scale = |input: &ID2D1Image, factor: f32, mode: i32| -> Result<ID2D1Image> {
        let s = effect(dc, &CLSID_D2D1Scale, input)?;
        set(&s, D2D1_SCALE_PROP_SCALE.0, D2D1_PROPERTY_TYPE_VECTOR2, &floats(&[factor, factor]))?;
        set(&s, D2D1_SCALE_PROP_CENTER_POINT.0, D2D1_PROPERTY_TYPE_VECTOR2, &floats(&[r.left, r.top]))?;
        set_enum(&s, D2D1_SCALE_PROP_INTERPOLATION_MODE.0, mode)?;
        set_enum(&s, D2D1_SCALE_PROP_BORDER_MODE.0, D2D1_BORDER_MODE_HARD.0)?;
        output(&s)
    };
    let small = scale(&clamped(dc, input, r)?, 1.0 / b, D2D1_SCALE_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC.0)?;
    scale(&small, b, D2D1_SCALE_INTERPOLATION_MODE_NEAREST_NEIGHBOR.0)
}

impl Drawing {
    /// A drawing over `area` (whole-image pixels) of `screens`, made from picture `version`.
    pub fn new(gfx: &Gfx, area: (u32, u32, u32, u32), screens: &RgbaImage, version: u64) -> Result<Self> {
        let (_, _, w, h) = area;
        let stroke = {
            let props = D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
                transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
            };
            // SAFETY: plain creation call.
            unsafe { gfx.factory.CreateStrokeStyle(&props, None)? }
        };
        let base = target(gfx, w, h)?;
        Self::picture(gfx, &base, area, screens)?;
        Ok(Self {
            area,
            base,
            base_version: version,
            feeds: 0,
            result: target(gfx, w, h)?,
            mask: None,
            spare: None,
            scratch: None,
            letters: Vec::new(),
            stroke,
            made: None,
            translated: None,
            version: next_version(),
        })
    }

    /// Draws the part of `screens` under `area` into `base`: the image's rows go to the graphics card as they are,
    /// which turns them to BGRA.
    fn picture(gfx: &Gfx, base: &ID2D1Bitmap1, area: (u32, u32, u32, u32), screens: &RgbaImage) -> Result<()> {
        let source = gfx.bitmap(screens, area)?;
        draw_into(&gfx.dc, base, || {
            // SAFETY: plain drawing call with a bitmap of the same device.
            unsafe {
                gfx.dc.DrawImage(
                    &source,
                    None,
                    None,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    D2D1_COMPOSITE_MODE_SOURCE_COPY,
                );
            }
            Ok(())
        })
    }

    /// Takes picture `version` of the screens instead.
    pub fn set_screens(&mut self, gfx: &Gfx, screens: &RgbaImage, version: u64) -> Result<()> {
        Self::picture(gfx, &self.base, self.area, screens)?;
        self.base_version = version;
        self.feeds += 1;
        Ok(())
    }

    /// Takes a fresh picture of a part of the screens, copied on the graphics card.
    pub fn feed(&mut self, gfx: &Gfx, part: &ScreenPart) -> Result<()> {
        let (ax, ay, aw, ah) = self.area;
        let x0 = part.x.max(ax);
        let y0 = part.y.max(ay);
        let x1 = (part.x + part.width).min(ax + aw);
        let y1 = (part.y + part.height).min(ay + ah);
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a valid out-pointer.
        unsafe { part.texture.GetDesc(&mut desc) };
        let (sx, sy) = (part.from.0 + (x0 - part.x), part.from.1 + (y0 - part.y));
        let (w, h) = ((x1 - x0).min(desc.Width.saturating_sub(sx)), (y1 - y0).min(desc.Height.saturating_sub(sy)));
        if w == 0 || h == 0 || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Ok(());
        }
        // SAFETY: plain call.
        let surface: IDXGISurface = unsafe { self.base.GetSurface()? };
        let base: ID3D11Texture2D = surface.cast()?;
        let source = D3D11_BOX { left: sx, top: sy, front: 0, right: sx + w, bottom: sy + h, back: 1 };
        // SAFETY: both textures are BGRA on the editor's device; the box lies inside the source and, from
        // `(x0 - ax, y0 - ay)`, inside the base (both checked above). The base is not being drawn into (only between
        // `BeginDraw` and `EndDraw`).
        unsafe {
            let context = gfx.d3d.GetImmediateContext()?;
            context.CopySubresourceRegion(&base, 0, x0 - ax, y0 - ay, 0, &part.texture, 0, Some(&raw const source));
        }
        self.feeds += 1;
        Ok(())
    }

    /// Draws `layers` (whole-image pixels) again if they, the screens or `live` changed.
    pub fn update(&mut self, gfx: &Gfx, layers: &std::rc::Rc<Vec<Annotation>>, live: bool) -> Result<()> {
        let made = (std::rc::Rc::as_ptr(layers) as usize, self.base_version, self.feeds, live);
        if self.made == Some(made) {
            return Ok(());
        }
        self.made = None;
        let local = match &self.translated {
            Some((source, local)) if std::rc::Rc::ptr_eq(source, layers) => local.clone(),
            _ => {
                let (ax, ay, _, _) = self.area;
                let local: Layers =
                    std::rc::Rc::new(layers.iter().map(|a| a.translated(-(ax as f32), -(ay as f32))).collect());
                self.make_letters(gfx, &local)?;
                self.translated = Some((layers.clone(), local.clone()));
                local
            }
        };
        self.compose(gfx, &local, live)?;
        self.made = Some(made);
        self.version = next_version();
        Ok(())
    }

    fn compose(&mut self, gfx: &Gfx, layers: &[Annotation], live: bool) -> Result<()> {
        let (_, _, w, h) = self.area;
        let dc = &gfx.dc;
        let uses_below = layers
            .iter()
            .any(|a| matches!(a, Annotation::Blur { .. } | Annotation::Pixelate { .. } | Annotation::Highlight { .. }));
        // The bitmaps no longer needed go (one more may come with the next effect).
        if !uses_below {
            self.spare = None;
        } else if self.spare.is_none() {
            self.spare = Some(target(gfx, w, h)?);
        }
        let needs_scratch = live || layers.iter().any(|a| matches!(a, Annotation::Highlight { .. }));
        if !needs_scratch {
            self.scratch = None;
        } else if self.scratch.is_none() {
            self.scratch = Some(target(gfx, w, h)?);
        }
        if !live {
            self.mask = None;
        } else if self.mask.is_none() {
            self.mask = Some(target(gfx, w, h)?);
        }
        let result = &self.result;
        let base: ID2D1Image = self.base.cast()?;
        // The effects are opaque where they are drawn: drawn over, they replace what was there (a copy mode would also
        // clear what lies outside them).
        draw_into(dc, result, || {
            // SAFETY: plain drawing calls.
            unsafe {
                dc.Clear(Some(&D2D1_COLOR_F::default()));
                dc.DrawImage(
                    &base,
                    None,
                    None,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
            }
            Ok(())
        })?;
        // Consecutive annotations drawn plainly share one drawing.
        let mut plain: Vec<&Annotation> = Vec::new();
        for a in layers {
            let below = match a {
                Annotation::Blur { rect, .. } | Annotation::Pixelate { rect, .. } => {
                    effect_rect(rect, w, h).map(|r| (r, a))
                }
                Annotation::Highlight { .. } => {
                    Some((D2D_RECT_F { left: 0.0, top: 0.0, right: w as f32, bottom: h as f32 }, a))
                }
                _ => None,
            };
            let Some((r, a)) = below else {
                if !matches!(a, Annotation::Blur { .. } | Annotation::Pixelate { .. }) {
                    plain.push(a);
                }
                continue;
            };
            self.draw_plain(dc, &mut plain)?;
            let (Some(spare), result) = (&self.spare, &self.result) else {
                continue;
            };
            // SAFETY: both bitmaps have the area's size and format; neither is a target now.
            unsafe { spare.CopyFromBitmap(None, result, None)? };
            let copy: ID2D1Image = spare.cast()?;
            if let Annotation::Highlight { .. } = a {
                let Some(scratch) = &self.scratch else {
                    continue;
                };
                draw_into(dc, scratch, || {
                    // SAFETY: plain drawing calls.
                    unsafe { dc.Clear(Some(&D2D1_COLOR_F::default())) };
                    self.shape(dc, a, None)
                })?;
                let blend = effect(dc, &CLSID_D2D1Blend, &copy)?;
                // SAFETY: plain property call with a live bitmap.
                unsafe { blend.SetInput(1, &scratch.cast::<ID2D1Image>()?, true) };
                set_enum(&blend, D2D1_BLEND_PROP_MODE.0, D2D1_BLEND_MODE_MULTIPLY.0)?;
                let out = output(&blend)?;
                draw_into(dc, result, || {
                    // SAFETY: plain drawing call.
                    unsafe {
                        dc.DrawImage(
                            &out,
                            None,
                            Some(&r),
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            D2D1_COMPOSITE_MODE_SOURCE_OVER,
                        );
                    }
                    Ok(())
                })?;
                continue;
            }
            let out = match a {
                Annotation::Blur { radius, .. } if *radius > 0.0 => blurred(dc, &copy, r, *radius)?,
                Annotation::Pixelate { block, .. } => pixelated(dc, &copy, r, *block)?,
                _ => continue,
            };
            draw_into(dc, result, || {
                // SAFETY: plain drawing call: the effect's pixels inside `r`, in place.
                unsafe {
                    dc.DrawImage(
                        &out,
                        Some(&v2(r.left, r.top)),
                        Some(&r),
                        D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                }
                Ok(())
            })?;
        }
        self.draw_plain(dc, &mut plain)?;
        if live {
            self.make_mask(gfx, layers, w, h)?;
        }
        Ok(())
    }

    /// Draws the annotations of `plain` on the result, and empties it.
    fn draw_plain(&self, dc: &ID2D1DeviceContext, plain: &mut Vec<&Annotation>) -> Result<()> {
        if plain.is_empty() {
            return Ok(());
        }
        let shapes: Vec<&Annotation> = std::mem::take(plain);
        draw_into(dc, &self.result, || {
            for a in shapes {
                self.shape(dc, a, None)?;
            }
            Ok(())
        })
    }

    /// Where something is drawn: each annotation in white, then made fully opaque wherever any of it shows.
    fn make_mask(&self, gfx: &Gfx, layers: &[Annotation], w: u32, h: u32) -> Result<()> {
        let (Some(scratch), Some(mask)) = (&self.scratch, &self.mask) else {
            return Ok(());
        };
        let dc = &gfx.dc;
        let white = Color::rgb(255, 255, 255);
        draw_into(dc, scratch, || {
            // SAFETY: plain drawing call.
            unsafe { dc.Clear(Some(&D2D1_COLOR_F::default())) };
            for a in layers {
                match a {
                    Annotation::Blur { rect, .. } | Annotation::Pixelate { rect, .. } => {
                        if let Some(r) = effect_rect(rect, w, h) {
                            // SAFETY: plain drawing calls.
                            unsafe {
                                let brush = dc.CreateSolidColorBrush(&colour(white), None)?;
                                dc.FillRectangle(&r, &brush);
                            }
                        }
                    }
                    other => self.shape(dc, other, Some(white))?,
                }
            }
            Ok(())
        })?;
        let threshold = effect(dc, &CLSID_D2D1ColorMatrix, &scratch.cast::<ID2D1Image>()?)?;
        #[rustfmt::skip]
        let matrix = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 255.0,
            0.0, 0.0, 0.0, 0.0,
        ];
        set(&threshold, D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0, D2D1_PROPERTY_TYPE_MATRIX_5X4, &floats(&matrix))?;
        set(&threshold, D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT.0, D2D1_PROPERTY_TYPE_BOOL, &1u32.to_le_bytes())?;
        let out = output(&threshold)?;
        draw_into(dc, mask, || {
            // SAFETY: plain drawing calls.
            unsafe {
                dc.Clear(Some(&D2D1_COLOR_F::default()));
                dc.DrawImage(
                    &out,
                    None,
                    None,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
            }
            Ok(())
        })
    }

    /// The bitmaps of the letters of `layers`, kept from the last drawing when unchanged or only moved.
    fn make_letters(&mut self, gfx: &Gfx, layers: &[Annotation]) -> Result<()> {
        let mut old = std::mem::take(&mut self.letters);
        for a in layers {
            let Some((key, _)) = letters_key(a) else {
                continue;
            };
            if let Some(i) = old.iter().position(|(was, _)| *was == key) {
                self.letters.push(old.swap_remove(i));
                continue;
            }
            if self.letters.iter().any(|(was, _)| *was == key) {
                continue;
            }
            let made = match letters(&key) {
                Some((image, x, y)) => Some(Letters {
                    bitmap: gfx.rgba_bitmap(image.width, image.height, &image.data)?,
                    at: D2D_RECT_F {
                        left: x as f32,
                        top: y as f32,
                        right: (x + image.width as i32) as f32,
                        bottom: (y + image.height as i32) as f32,
                    },
                }),
                None => None,
            };
            self.letters.push((key, made));
        }
        Ok(())
    }

    fn path(&self, dc: &ID2D1DeviceContext, points: &[Point]) -> Result<Option<ID2D1PathGeometry>> {
        let Some((first, segments)) = smooth(points) else {
            return Ok(None);
        };
        // SAFETY: plain geometry calls; the sink is closed before the geometry is used.
        unsafe {
            let factory = dc.GetFactory()?;
            let geometry = factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(v2(first.x, first.y), D2D1_FIGURE_BEGIN_HOLLOW);
            for s in segments {
                match s {
                    Segment::Line(p) => sink.AddLine(v2(p.x, p.y)),
                    Segment::Quad(c, p) => sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                        point1: v2(c.x, c.y),
                        point2: v2(p.x, p.y),
                    }),
                }
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            Ok(Some(geometry))
        }
    }

    /// Draws `a` (area pixels) on the current target, in its colours or all in `paint`.
    fn shape(&self, dc: &ID2D1DeviceContext, a: &Annotation, paint: Option<Color>) -> Result<()> {
        let brush = |c: Color| {
            // SAFETY: plain creation call.
            unsafe { dc.CreateSolidColorBrush(&colour(paint.unwrap_or(c)), None) }
        };
        let stroke = |points: &[Point], width: f32, c: Color| -> Result<()> {
            if let Some(path) = self.path(dc, points)? {
                // SAFETY: plain drawing call with live resources.
                unsafe { dc.DrawGeometry(&path, &brush(c)?, stroke_width(width), &self.stroke) };
            }
            Ok(())
        };
        let letters = |a: &Annotation| {
            let Some((key, (dx, dy))) = letters_key(a) else {
                return;
            };
            if let Some((_, Some(l))) = self.letters.iter().find(|(was, _)| *was == key) {
                let at = D2D_RECT_F {
                    left: l.at.left + dx,
                    top: l.at.top + dy,
                    right: l.at.right + dx,
                    bottom: l.at.bottom + dy,
                };
                // SAFETY: plain drawing call with a live bitmap.
                unsafe {
                    dc.DrawBitmap(&l.bitmap, Some(&at), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
                }
            }
        };
        match a {
            Annotation::Pen { points, style } => stroke(points, style.width, style.color)?,
            Annotation::Highlight { points, color, width } => stroke(points, *width, *color)?,
            Annotation::Line { from, to, style } => stroke(&[*from, *to], style.width, style.color)?,
            Annotation::Arrow { from, to, style } => {
                if let Some(shape) = arrow_shape(*from, *to, style.width) {
                    stroke(&[*from, shape.shaft_end], style.width, style.color)?;
                    let [p, q, r] = shape.head;
                    // SAFETY: plain geometry and drawing calls; the sink is closed before use.
                    unsafe {
                        let geometry = dc.GetFactory()?.CreatePathGeometry()?;
                        let sink = geometry.Open()?;
                        sink.BeginFigure(v2(p.x, p.y), D2D1_FIGURE_BEGIN_FILLED);
                        sink.AddLine(v2(q.x, q.y));
                        sink.AddLine(v2(r.x, r.y));
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        sink.Close()?;
                        dc.FillGeometry(&geometry, &brush(style.color)?, None);
                    }
                }
            }
            Annotation::Rect { rect, style, filled } => {
                let r = rect_f(rect);
                let b = brush(style.color)?;
                // SAFETY: plain drawing calls.
                unsafe {
                    if *filled {
                        dc.FillRectangle(&r, &b);
                    } else {
                        dc.DrawRectangle(&r, &b, stroke_width(style.width), &self.stroke);
                    }
                }
            }
            Annotation::Ellipse { rect, style, filled } => {
                let r = rect_f(rect);
                let e = D2D1_ELLIPSE {
                    point: v2(f32::midpoint(r.left, r.right), f32::midpoint(r.top, r.bottom)),
                    radiusX: (r.right - r.left) / 2.0,
                    radiusY: (r.bottom - r.top) / 2.0,
                };
                let b = brush(style.color)?;
                // SAFETY: plain drawing calls.
                unsafe {
                    if *filled {
                        dc.FillEllipse(&e, &b);
                    } else {
                        dc.DrawEllipse(&e, &b, stroke_width(style.width), &self.stroke);
                    }
                }
            }
            Annotation::Text { .. } => {
                if let Some((r, bg)) = text_background(a) {
                    // SAFETY: plain drawing call.
                    unsafe { dc.FillRectangle(&rect_f(&r), &brush(bg)?) };
                }
                letters(a);
            }
            Annotation::Marker { center, color, radius, .. } => {
                let e = D2D1_ELLIPSE { point: v2(center.x, center.y), radiusX: *radius, radiusY: *radius };
                // SAFETY: plain drawing call.
                unsafe { dc.FillEllipse(&e, &brush(*color)?) };
                letters(a);
            }
            Annotation::Blur { .. } | Annotation::Pixelate { .. } => {}
        }
        Ok(())
    }

    /// Draws the result on `c`, its top-left corner at `at`; live, only where something is drawn (the screens show
    /// elsewhere).
    pub fn draw(&self, c: &Canvas<'_>, at: (f32, f32), live: bool) {
        let offset = v2(at.0, at.1);
        let place = |image: &ID2D1Bitmap1, mode: D2D1_COMPOSITE_MODE| {
            // SAFETY: plain drawing call with a bitmap of the same device.
            unsafe {
                c.dc.DrawImage(image, Some(&offset), None, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, mode);
            }
        };
        place(&self.result, D2D1_COMPOSITE_MODE_SOURCE_OVER);
        if live && let Some(mask) = &self.mask {
            place(mask, D2D1_COMPOSITE_MODE_DESTINATION_IN);
        }
    }
}
