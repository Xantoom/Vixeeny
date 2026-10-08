// SPDX-License-Identifier: GPL-3.0-or-later
//! The devices (Direct3D 11 → Direct2D → DirectComposition, DirectWrite for text) and a small
//! drawing API over a Direct2D context: rounded boxes, text, icons, shadows, bitmaps. The editor
//! and the popups (side strip, notification, recording widget) draw with it.

use std::cell::RefCell;
use std::collections::HashMap;

use vixeeny_editor::RgbaImage;
use windows::Win32::Foundation::{HMODULE, POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED,
    D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_WINDING, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1Shadow, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_MAP_OPTIONS_READ, D2D1_PROPERTY_TYPE_FLOAT,
    D2D1_PROPERTY_TYPE_VECTOR4, D2D1_ROUNDED_RECT, D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION,
    D2D1_SHADOW_PROP_COLOR, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Bitmap1,
    ID2D1Brush, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1, ID2D1Geometry, ID2D1Image,
    ID2D1Layer, ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionSurface,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory,
    IDWriteFactory5, IDWriteFontCollection, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::core::{HSTRING, Interface, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::svg::{self, Segment};
use crate::theme::Rgba;

/// The font of the interface: Segoe UI Variable (Windows 11), which falls back to Segoe UI.
const UI_FONT: &str = "Segoe UI Variable Text";
/// The font the editor draws text annotations with (also in `vixeeny-editor`).
static INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const INTER_FAMILY: &str = "Inter";

pub fn color(c: Rgba) -> D2D1_COLOR_F {
    let [r, g, b, a] = c.0;
    D2D1_COLOR_F { r, g, b, a }
}

/// `(x, y, w, h)` → a Direct2D rectangle.
pub fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Ui,
    /// The annotations' font (Inter), so typed text looks as it will be drawn.
    Inter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Leading,
    Centre,
}

pub struct Gfx {
    pub d3d: ID3D11Device,
    pub factory: ID2D1Factory1,
    pub d2d: ID2D1Device,
    /// For resources that outlive one drawing (bitmaps) and for drawing off screen.
    pub dc: ID2D1DeviceContext,
    pub dcomp: IDCompositionDesktopDevice,
    dwrite: IDWriteFactory,
    inter: Option<IDWriteFontCollection>,
    icons: RefCell<HashMap<usize, Option<ID2D1PathGeometry>>>,
}

fn d3d_device(driver: D3D_DRIVER_TYPE) -> Result<ID3D11Device> {
    let mut device = None;
    // SAFETY: the out-pointer is valid for the call; no adapter, default feature levels.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            None,
        )?;
    }
    device.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
}

/// The Inter font as a DirectWrite collection (from the bytes in the binary).
fn inter_collection(dwrite: &IDWriteFactory) -> Result<IDWriteFontCollection> {
    let factory: IDWriteFactory5 = dwrite.cast()?;
    // SAFETY: `INTER` is a static, so the memory the font file refers to lives as long as the
    // process; the loader is registered before it is used.
    unsafe {
        let loader = factory.CreateInMemoryFontFileLoader()?;
        factory.RegisterFontFileLoader(&loader)?;
        let file = loader.CreateInMemoryFontFileReference(
            &factory,
            INTER.as_ptr().cast(),
            u32::try_from(INTER.len()).unwrap_or(u32::MAX),
            None,
        )?;
        let builder = factory.CreateFontSetBuilder()?;
        builder.AddFontFile(&file)?;
        let set = builder.CreateFontSet()?;
        let collection = factory.CreateFontCollectionFromFontSet(&set)?;
        collection.cast()
    }
}

impl Gfx {
    pub fn new() -> Result<Self> {
        let d3d = d3d_device(D3D_DRIVER_TYPE_HARDWARE).or_else(|e| {
            tracing::warn!("no hardware Direct3D device ({e}), using WARP");
            d3d_device(D3D_DRIVER_TYPE_WARP)
        })?;
        let dxgi: IDXGIDevice = d3d.cast()?;
        // SAFETY: plain factory and device creation on live objects.
        let (factory, d2d, dc, dcomp, dwrite) = unsafe {
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d: ID2D1Device = factory.CreateDevice(&dxgi)?;
            let dc = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dcomp: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            (factory, d2d, dc, dcomp, dwrite)
        };
        let inter = inter_collection(&dwrite)
            .map_err(|e| tracing::warn!("the Inter font is not available: {e}"))
            .ok();
        Ok(Self {
            d3d,
            factory,
            d2d,
            dc,
            dcomp,
            dwrite,
            inter,
            icons: RefCell::default(),
        })
    }

    /// A transparent surface of `w`×`h` pixels for drawn content.
    pub fn surface(&self, w: u32, h: u32) -> Result<IDCompositionSurface> {
        // SAFETY: plain creation call.
        unsafe {
            self.dcomp.CreateSurface(
                w.max(1),
                h.max(1),
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )
        }
    }

    /// An opaque surface (pictures).
    pub fn opaque_surface(&self, w: u32, h: u32) -> Result<IDCompositionSurface> {
        // SAFETY: plain creation call.
        unsafe {
            self.dcomp.CreateSurface(
                w.max(1),
                h.max(1),
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_IGNORE,
            )
        }
    }

    /// Draws the whole of `surface` (`w`×`h`), cleared first.
    pub fn draw(
        &self,
        surface: &IDCompositionSurface,
        w: u32,
        h: u32,
        f: impl FnOnce(&Canvas<'_>) -> Result<()>,
    ) -> Result<()> {
        let update = RECT {
            left: 0,
            top: 0,
            right: w.max(1) as i32,
            bottom: h.max(1) as i32,
        };
        let mut offset = POINT::default();
        // SAFETY: `update` lies inside the surface; the context returned is only used until
        // `EndDraw`, which is always called.
        let dc: ID2D1DeviceContext = unsafe { surface.BeginDraw(Some(&update), &mut offset)? };
        // SAFETY: plain state calls on the context DirectComposition gave.
        unsafe {
            dc.SetDpi(96.0, 96.0);
            dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            dc.Clear(Some(&D2D1_COLOR_F::default()));
        }
        let result = f(&Canvas { dc: &dc, gfx: self });
        // SAFETY: matches the successful `BeginDraw` above.
        let ended = unsafe { surface.EndDraw() };
        result.and(ended)
    }

    /// A picture: `img`'s `(x, y, w, h)` part, 1:1.
    pub fn picture(
        &self,
        img: &RgbaImage,
        part: (u32, u32, u32, u32),
    ) -> Result<IDCompositionSurface> {
        let (_, _, w, h) = part;
        let surface = self.opaque_surface(w, h)?;
        let bitmap = self.bitmap(img, part)?;
        self.draw(&surface, w, h, |c| {
            // SAFETY: plain drawing call with a live bitmap.
            unsafe {
                c.dc.DrawBitmap(
                    &bitmap,
                    Some(&rect(0.0, 0.0, w as f32, h as f32)),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
            Ok(())
        })?;
        Ok(surface)
    }

    /// `img`'s `(x, y, w, h)` part as a Direct2D bitmap (opaque).
    pub fn bitmap(&self, img: &RgbaImage, part: (u32, u32, u32, u32)) -> Result<ID2D1Bitmap1> {
        let (x, y, w, h) = part;
        let stride = img.width as usize * 4;
        let start = y as usize * stride + x as usize * 4;
        let end = start + (h.max(1) as usize - 1) * stride + w as usize * 4;
        let Some(pixels) = img.data.get(start..end) else {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        };
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_R8G8B8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        // SAFETY: `pixels` holds `h` rows `stride` bytes apart, each with `w` RGBA pixels.
        unsafe {
            self.dc.CreateBitmap(
                D2D_SIZE_U {
                    width: w.max(1),
                    height: h.max(1),
                },
                Some(pixels.as_ptr().cast()),
                u32::try_from(stride).unwrap_or(u32::MAX),
                &props,
            )
        }
    }

    fn format(
        &self,
        size: f32,
        font: Font,
        weight: DWRITE_FONT_WEIGHT,
    ) -> Result<windows::Win32::Graphics::DirectWrite::IDWriteTextFormat> {
        let (family, collection) = match (font, &self.inter) {
            (Font::Inter, Some(c)) => (INTER_FAMILY, Some(c)),
            _ => (UI_FONT, None),
        };
        // SAFETY: plain creation call; the strings outlive it.
        unsafe {
            self.dwrite.CreateTextFormat(
                &HSTRING::from(family),
                collection,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size.max(1.0),
                w!("en-us"),
            )
        }
    }

    /// `text` laid out on one line at `size` pixels.
    pub fn layout(&self, text: &str, size: f32, font: Font) -> Result<IDWriteTextLayout> {
        let format = self.format(size, font, DWRITE_FONT_WEIGHT_NORMAL)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: plain calls on live objects.
        unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            if font == Font::Inter {
                // The editor's own line metrics, so the field matches the drawn text.
                let ascent = vixeeny_editor::text::font()
                    .horizontal_line_metrics(size)
                    .map_or(size * 0.95, |m| m.ascent);
                format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, size * 1.25, ascent)?;
            }
            self.dwrite
                .CreateTextLayout(&wide, &format, 100_000.0, 100_000.0)
        }
    }

    /// Width and height of `text` on one line.
    pub fn measure(&self, text: &str, size: f32, font: Font) -> Result<(f32, f32)> {
        let layout = self.layout(text, size, font)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: the out-pointer is valid.
        unsafe { layout.GetMetrics(&mut m)? };
        Ok((m.widthIncludingTrailingWhitespace, m.height))
    }

    /// `text` in the interface font at `size` pixels, wrapped at `width` (one line when it fits),
    /// semi-bold or regular, with its size.
    pub fn paragraph(&self, text: &str, size: f32, bold: bool, width: f32) -> Result<Paragraph> {
        let weight = if bold {
            DWRITE_FONT_WEIGHT_SEMI_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        let format = self.format(size, Font::Ui, weight)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: plain calls on live objects; the out-pointer is valid.
        let layout = unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            let layout = self
                .dwrite
                .CreateTextLayout(&wide, &format, width.max(1.0), 100_000.0)?;
            layout.GetMetrics(&mut m)?;
            layout
        };
        Ok(Paragraph {
            layout,
            width: m.widthIncludingTrailingWhitespace,
            height: m.height,
        })
    }

    /// An RGBA picture (straight alpha, `w`×`h`) as a Direct2D bitmap.
    pub fn rgba_bitmap(&self, w: u32, h: u32, rgba: &[u8]) -> Result<ID2D1Bitmap1> {
        if rgba.len() != w as usize * h as usize * 4 || w == 0 || h == 0 {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        let mut premultiplied = rgba.to_vec();
        for px in premultiplied.as_chunks_mut::<4>().0 {
            let a = u16::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_R8G8B8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        // SAFETY: `premultiplied` holds `h` rows of `w` RGBA pixels, `w * 4` bytes apart.
        unsafe {
            self.dc.CreateBitmap(
                D2D_SIZE_U {
                    width: w,
                    height: h,
                },
                Some(premultiplied.as_ptr().cast()),
                w * 4,
                &props,
            )
        }
    }

    /// The x of the caret after `chars` UTF-16 units of `layout`.
    pub fn caret_x(layout: &IDWriteTextLayout, position: u32) -> f32 {
        let (mut x, mut y) = (0.0, 0.0);
        let mut metrics = Default::default();
        // SAFETY: the out-pointers are valid.
        let ok =
            unsafe { layout.HitTestTextPosition(position, false, &mut x, &mut y, &mut metrics) };
        if ok.is_ok() { x } else { 0.0 }
    }

    fn icon(&self, data: &'static str) -> Option<ID2D1PathGeometry> {
        let key = data.as_ptr() as usize;
        if let Some(g) = self.icons.borrow().get(&key) {
            return g.clone();
        }
        let geometry = svg::parse(data).and_then(|figures| self.geometry(&figures).ok());
        self.icons.borrow_mut().insert(key, geometry.clone());
        geometry
    }

    fn geometry(&self, figures: &[svg::Figure]) -> Result<ID2D1PathGeometry> {
        // SAFETY: plain calls on a new geometry and its sink, closed before use.
        unsafe {
            let geometry: ID2D1PathGeometry = self.factory.CreatePathGeometry()?.cast()?;
            let sink = geometry.Open()?;
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            for f in figures {
                sink.BeginFigure(v2(f.start.0, f.start.1), D2D1_FIGURE_BEGIN_FILLED);
                for s in &f.segments {
                    match *s {
                        Segment::Line(x, y) => sink.AddLine(v2(x, y)),
                        Segment::Cubic(c) => sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                            point1: v2(c[0], c[1]),
                            point2: v2(c[2], c[3]),
                            point3: v2(c[4], c[5]),
                        }),
                    }
                }
                sink.EndFigure(if f.closed {
                    D2D1_FIGURE_END_CLOSED
                } else {
                    D2D1_FIGURE_END_OPEN
                });
            }
            sink.Close()?;
            Ok(geometry)
        }
    }

    /// Draws off screen into a `w`×`h` bitmap and returns its premultiplied BGRA pixels (tests).
    pub fn render(
        &self,
        w: u32,
        h: u32,
        f: impl FnOnce(&Canvas<'_>) -> Result<()>,
    ) -> Result<Vec<u8>> {
        let props = |options| D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: options,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let size = D2D_SIZE_U {
            width: w,
            height: h,
        };
        let dc = &self.dc;
        // SAFETY: the bitmaps are created, drawn and read on this thread; the mapped memory is
        // read within `Map`/`Unmap`, row by row within `pitch × h`.
        unsafe {
            let target = dc.CreateBitmap(size, None, 0, &props(D2D1_BITMAP_OPTIONS_TARGET))?;
            dc.SetTarget(&target);
            dc.SetDpi(96.0, 96.0);
            dc.BeginDraw();
            dc.SetTransform(&Matrix3x2::identity());
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.Clear(Some(&D2D1_COLOR_F::default()));
            let drawn = f(&Canvas { dc, gfx: self });
            dc.EndDraw(None, None)?;
            dc.SetTarget(None::<&ID2D1Image>);
            drawn?;
            let read = dc.CreateBitmap(
                size,
                None,
                0,
                &props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
            )?;
            read.CopyFromBitmap(None, &target, None)?;
            let mapped = read.Map(D2D1_MAP_OPTIONS_READ)?;
            let mut out = Vec::with_capacity(w as usize * h as usize * 4);
            for row in 0..h as usize {
                let line = std::slice::from_raw_parts(
                    mapped.bits.add(row * mapped.pitch as usize),
                    w as usize * 4,
                );
                out.extend_from_slice(line);
            }
            read.Unmap()?;
            Ok(out)
        }
    }
}

/// Laid out text and its size, in pixels.
pub struct Paragraph {
    pub layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
}

/// Drawing on a Direct2D context, in pixels.
pub struct Canvas<'a> {
    pub dc: &'a ID2D1DeviceContext,
    pub gfx: &'a Gfx,
}

impl Canvas<'_> {
    pub fn brush(&self, c: Rgba) -> Result<ID2D1SolidColorBrush> {
        // SAFETY: plain creation call.
        unsafe { self.dc.CreateSolidColorBrush(&color(c), None) }
    }

    pub fn fill_rect(&self, r: D2D_RECT_F, c: Rgba) -> Result<()> {
        let brush = self.brush(c)?;
        // SAFETY: plain drawing call.
        unsafe { self.dc.FillRectangle(&r, &brush) };
        Ok(())
    }

    pub fn fill_round(&self, r: D2D_RECT_F, radius: f32, c: Rgba) -> Result<()> {
        let brush = self.brush(c)?;
        let rounded = D2D1_ROUNDED_RECT {
            rect: r,
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: plain drawing call.
        unsafe { self.dc.FillRoundedRectangle(&rounded, &brush) };
        Ok(())
    }

    /// A border of `width` inside `r` (as CSS and Slint draw borders).
    pub fn stroke_round(&self, r: D2D_RECT_F, radius: f32, width: f32, c: Rgba) -> Result<()> {
        let brush = self.brush(c)?;
        let half = width / 2.0;
        let rounded = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: r.left + half,
                top: r.top + half,
                right: r.right - half,
                bottom: r.bottom - half,
            },
            radiusX: (radius - half).max(0.0),
            radiusY: (radius - half).max(0.0),
        };
        // SAFETY: plain drawing call.
        unsafe { self.dc.DrawRoundedRectangle(&rounded, &brush, width, None) };
        Ok(())
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, c: Rgba) -> Result<()> {
        let brush = self.brush(c)?;
        let e = D2D1_ELLIPSE {
            point: v2(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: plain drawing call.
        unsafe { self.dc.FillEllipse(&e, &brush) };
        Ok(())
    }

    /// A ring of `width` inside the circle of `radius`.
    pub fn stroke_circle(&self, cx: f32, cy: f32, radius: f32, width: f32, c: Rgba) -> Result<()> {
        let brush = self.brush(c)?;
        let r = radius - width / 2.0;
        let e = D2D1_ELLIPSE {
            point: v2(cx, cy),
            radiusX: r,
            radiusY: r,
        };
        // SAFETY: plain drawing call.
        unsafe { self.dc.DrawEllipse(&e, &brush, width, None) };
        Ok(())
    }

    /// `text` in the box `r`, vertically centred, leading or centred.
    pub fn text(&self, text: &str, r: D2D_RECT_F, size: f32, c: Rgba, align: Align) -> Result<()> {
        let format = self.gfx.format(size, Font::Ui, DWRITE_FONT_WEIGHT_NORMAL)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let brush = self.brush(c)?;
        // SAFETY: plain calls on live objects.
        unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetTextAlignment(match align {
                Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Centre => DWRITE_TEXT_ALIGNMENT_CENTER,
            })?;
            let layout = self.gfx.dwrite.CreateTextLayout(
                &wide,
                &format,
                (r.right - r.left).max(0.0),
                (r.bottom - r.top).max(0.0),
            )?;
            self.dc.DrawTextLayout(
                v2(r.left, r.top),
                &layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
        Ok(())
    }

    /// A laid out text with its top-left corner at `(x, y)`.
    pub fn text_layout(&self, layout: &IDWriteTextLayout, x: f32, y: f32, c: Rgba) -> Result<()> {
        let brush = self.brush(c)?;
        // SAFETY: plain drawing call.
        unsafe {
            self.dc
                .DrawTextLayout(v2(x, y), layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        Ok(())
    }

    /// An icon (20×20 outline) drawn `size` pixels wide at `(x, y)`.
    pub fn icon(&self, data: &'static str, x: f32, y: f32, size: f32, c: Rgba) -> Result<()> {
        let Some(geometry) = self.gfx.icon(data) else {
            return Ok(());
        };
        let brush = self.brush(c)?;
        let mut old = Matrix3x2::identity();
        // SAFETY: plain state and drawing calls; the transform is restored.
        unsafe {
            self.dc.GetTransform(&mut old);
            let m = Matrix3x2::scale(size / 20.0, size / 20.0) * Matrix3x2::translation(x, y) * old;
            self.dc.SetTransform(&m);
            self.dc.FillGeometry(&geometry, &brush, None::<&ID2D1Brush>);
            self.dc.SetTransform(&old);
        }
        Ok(())
    }

    /// The soft shadow of a rounded box (`blur` as in CSS, `dy` down).
    pub fn shadow(&self, r: D2D_RECT_F, radius: f32, blur: f32, dy: f32, c: Rgba) -> Result<()> {
        // SAFETY: the context's target is switched to a command list and back; the effect reads
        // the closed list.
        unsafe {
            let list = self.dc.CreateCommandList()?;
            let target = self.dc.GetTarget()?;
            let mut old = Matrix3x2::identity();
            self.dc.GetTransform(&mut old);
            self.dc.SetTarget(&list);
            self.dc.SetTransform(&Matrix3x2::identity());
            let brush = self.brush(Rgba([0.0, 0.0, 0.0, 1.0]))?;
            self.dc.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: r,
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            );
            self.dc.SetTarget(&target);
            self.dc.SetTransform(&old);
            list.Close()?;
            let effect = self.dc.CreateEffect(&CLSID_D2D1Shadow)?;
            effect.SetInput(0, &list, true);
            let deviation = blur / 2.0;
            effect.SetValue(
                D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &deviation.to_le_bytes(),
            )?;
            let mut rgba = Vec::with_capacity(16);
            for v in c.0 {
                rgba.extend_from_slice(&v.to_le_bytes());
            }
            effect.SetValue(
                D2D1_SHADOW_PROP_COLOR.0 as u32,
                D2D1_PROPERTY_TYPE_VECTOR4,
                &rgba,
            )?;
            let output = effect.GetOutput()?;
            self.dc.DrawImage(
                &output,
                Some(&v2(0.0, dy)),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
        Ok(())
    }

    /// `bitmap` stretched over `r` without smoothing.
    pub fn pixels(&self, bitmap: &ID2D1Bitmap1, r: D2D_RECT_F) -> Result<()> {
        // SAFETY: plain drawing call.
        unsafe {
            self.dc.DrawBitmap(
                bitmap,
                Some(&r),
                1.0,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                None,
                None,
            );
        }
        Ok(())
    }

    /// `bitmap` scaled smoothly over `r`.
    pub fn picture(&self, bitmap: &ID2D1Bitmap1, r: D2D_RECT_F) -> Result<()> {
        // SAFETY: plain drawing call.
        unsafe {
            self.dc.DrawBitmap(
                bitmap,
                Some(&r),
                1.0,
                D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                None,
                None,
            );
        }
        Ok(())
    }

    /// Clips what follows to the rounded box `r` until [`Canvas::pop_layer`].
    pub fn push_round_clip(&self, r: D2D_RECT_F, radius: f32) -> Result<()> {
        // SAFETY: the geometry is created on the context's factory and kept by the layer until
        // `pop_layer`.
        unsafe {
            let mask = self
                .gfx
                .factory
                .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                    rect: r,
                    radiusX: radius,
                    radiusY: radius,
                })?;
            let mask: ID2D1Geometry = mask.cast()?;
            let params = D2D1_LAYER_PARAMETERS1 {
                contentBounds: rect(-1e6, -1e6, 2e6, 2e6),
                geometricMask: std::mem::ManuallyDrop::new(Some(mask)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: 1.0,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            self.dc.PushLayer(&params, None::<&ID2D1Layer>);
            // The layer holds its own reference.
            drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
        }
        Ok(())
    }

    pub fn pop_layer(&self) {
        // SAFETY: pairs a `push_round_clip`.
        unsafe { self.dc.PopLayer() };
    }

    /// A linear gradient over `r` from `(x0, y0)` to `(x1, y1)` through `stops`.
    pub fn gradient(
        &self,
        r: D2D_RECT_F,
        radius: f32,
        from: (f32, f32),
        to: (f32, f32),
        stops: &[(f32, Rgba)],
    ) -> Result<()> {
        let stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|(position, c)| D2D1_GRADIENT_STOP {
                position: *position,
                color: color(*c),
            })
            .collect();
        // SAFETY: plain creation and drawing calls.
        unsafe {
            let target: &ID2D1RenderTarget = self.dc;
            let collection = target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let brush = self.dc.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: v2(from.0, from.1),
                    endPoint: v2(to.0, to.1),
                },
                None,
                &collection,
            )?;
            self.dc.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: r,
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            );
        }
        Ok(())
    }

    /// Clips what follows to `r` until [`Canvas::pop_clip`].
    pub fn push_clip(&self, r: D2D_RECT_F) {
        // SAFETY: plain state call, paired with `pop_clip`.
        unsafe {
            self.dc
                .PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
    }

    pub fn pop_clip(&self) {
        // SAFETY: pairs a `push_clip`.
        unsafe { self.dc.PopAxisAlignedClip() };
    }
}

/// An opaque 1×1 surface of colour `c`, to be scaled into rectangles by its visual.
pub fn solid(gfx: &Gfx, c: Rgba) -> Result<IDCompositionSurface> {
    let surface = gfx.surface(1, 1)?;
    gfx.draw(&surface, 1, 1, |canvas| {
        canvas.fill_rect(rect(0.0, 0.0, 1.0, 1.0), c)
    })?;
    Ok(surface)
}
