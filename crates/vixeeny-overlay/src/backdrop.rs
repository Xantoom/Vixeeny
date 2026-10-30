// SPDX-License-Identifier: GPL-3.0-or-later
//! A captured window set on its background by the graphics card, as `vixeeny_image::backdrop` does it on the processor
//! (same layout, same light): the margin, the blur or the colour behind, the soft shadow, the rounded corners. An SDR
//! picture crosses as 8-bit sRGB both ways and the card works in linear light in between; an HDR one crosses as scRGB
//! floats.

use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED,
    D2D1_BORDER_MODE_HARD, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_COPY, D2D1_COMPOSITE_MODE_SOURCE_OVER,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1ColorManagement, CLSID_D2D1Scale, CLSID_D2D1Shadow, D2D1_BITMAP_BRUSH_PROPERTIES1, D2D1_BITMAP_OPTIONS,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1, D2D1_BUFFER_PRECISION_32BPC_FLOAT,
    D2D1_COLOR_SPACE, D2D1_COLOR_SPACE_SCRGB, D2D1_COLOR_SPACE_SRGB,
    D2D1_COLORMANAGEMENT_PROP_DESTINATION_COLOR_CONTEXT, D2D1_COLORMANAGEMENT_PROP_QUALITY,
    D2D1_COLORMANAGEMENT_PROP_SOURCE_COLOR_CONTEXT, D2D1_COLORMANAGEMENT_QUALITY_BEST,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_PROPERTY_TYPE_COLOR_CONTEXT, D2D1_PROPERTY_TYPE_FLOAT,
    D2D1_PROPERTY_TYPE_VECTOR2, D2D1_PROPERTY_TYPE_VECTOR4, D2D1_ROUNDED_RECT, D2D1_SCALE_INTERPOLATION_MODE_LINEAR,
    D2D1_SCALE_PROP_BORDER_MODE, D2D1_SCALE_PROP_CENTER_POINT, D2D1_SCALE_PROP_INTERPOLATION_MODE,
    D2D1_SCALE_PROP_SCALE, D2D1_SHADOW_OPTIMIZATION_QUALITY, D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION,
    D2D1_SHADOW_PROP_COLOR, D2D1_SHADOW_PROP_OPTIMIZATION, D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext,
    ID2D1Factory1, ID2D1Image,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::core::{Interface, Result};
use windows_numerics::Matrix3x2;

use crate::drawing::{blurred, clamped, draw_into, effect, floats, output, set, set_enum};

/// What goes behind the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Behind {
    /// The window itself, enlarged and heavily blurred.
    Blur,
    /// A plain colour, in linear light (at SDR white for an HDR picture).
    Colour([f32; 3]),
}

/// How the window is set (sizes from `vixeeny_image::backdrop::layout`, so that both paths give the same picture).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    pub margin: u32,
    pub radius: f32,
    pub offset: u32,
    /// How far the shadow reaches past the window on every side.
    pub spread: u32,
    /// The radius of the three box blurs of the processor's shadow.
    pub softness: u32,
    pub small: (u32, u32),
    pub small_radius: u32,
    pub darken: f32,
    pub shadow: f32,
    pub behind: Behind,
}

/// The captured window.
pub enum Window<'a> {
    /// 8-bit BGRA, sRGB, rows `stride` bytes apart.
    Sdr { width: u32, height: u32, stride: usize, bgra: &'a [u8] },
    /// scRGB, 4 floats per pixel.
    Hdr { width: u32, height: u32, rgba: &'a [f32] },
}

/// The window on its background: tight BGRA for an SDR window, scRGB floats for an HDR one.
pub enum Set {
    Sdr(Vec<u8>),
    Hdr(Vec<f32>),
}

/// The standard deviation of three box blurs of `radius`, as one Gaussian blur.
fn sigma(radius: u32) -> f32 {
    let r = radius as f32;
    (r * (r + 1.0)).sqrt()
}

fn rect(w: u32, h: u32) -> D2D_RECT_F {
    D2D_RECT_F { left: 0.0, top: 0.0, right: w as f32, bottom: h as f32 }
}

fn props(format: DXGI_FORMAT, alpha: D2D1_ALPHA_MODE, options: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format, alphaMode: alpha },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        colorContext: std::mem::ManuallyDrop::new(None),
    }
}

fn target(dc: &ID2D1DeviceContext, (w, h): (u32, u32), format: DXGI_FORMAT) -> Result<ID2D1Bitmap1> {
    let size = D2D_SIZE_U { width: w, height: h };
    let props = props(format, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BITMAP_OPTIONS_TARGET);
    // SAFETY: plain creation call.
    unsafe { dc.CreateBitmap(size, None, 0, &props) }
}

/// `input` from one colour space to another (sRGB ↔ linear scRGB).
fn convert(
    dc: &ID2D1DeviceContext,
    input: &ID2D1Image,
    from: D2D1_COLOR_SPACE,
    to: D2D1_COLOR_SPACE,
) -> Result<ID2D1Image> {
    let e = effect(dc, &CLSID_D2D1ColorManagement, input)?;
    // SAFETY: the colour contexts are passed as interface pointers, as the property expects; the effect keeps its own
    // references.
    unsafe {
        let source = dc.CreateColorContext(from, None)?;
        let destination = dc.CreateColorContext(to, None)?;
        for (prop, context) in [
            (D2D1_COLORMANAGEMENT_PROP_SOURCE_COLOR_CONTEXT, &source),
            (D2D1_COLORMANAGEMENT_PROP_DESTINATION_COLOR_CONTEXT, &destination),
        ] {
            e.SetValue(prop.0 as u32, D2D1_PROPERTY_TYPE_COLOR_CONTEXT, &(context.as_raw() as usize).to_ne_bytes())?;
        }
    }
    set_enum(&e, D2D1_COLORMANAGEMENT_PROP_QUALITY.0, D2D1_COLORMANAGEMENT_QUALITY_BEST.0)?;
    output(&e)
}

/// A Direct2D context on `d3d` (made with BGRA support).
fn context(d3d: &ID3D11Device) -> Result<ID2D1DeviceContext> {
    let dxgi: IDXGIDevice = d3d.cast()?;
    // SAFETY: plain factory and device creation on live objects.
    unsafe {
        let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let device = factory.CreateDevice(&dxgi)?;
        let dc = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        let mut controls = dc.GetRenderingControls();
        controls.bufferPrecision = D2D1_BUFFER_PRECISION_32BPC_FLOAT;
        dc.SetRenderingControls(&controls);
        Ok(dc)
    }
}

/// `window` on its background, drawn by the graphics card on `d3d` (the device the window was captured on: making one
/// costs more than the drawing). `Err` when the card cannot, or without a device (the caller then does it on the
/// processor).
pub fn set_on_card(d3d: Option<&ID3D11Device>, window: &Window<'_>, setting: &Setting) -> Result<Set> {
    let d3d = d3d.ok_or_else(|| windows::core::Error::from(E_NOTIMPL))?;
    let dc = context(d3d)?;
    let (ww, wh, hdr) = match window {
        Window::Sdr { width, height, .. } => (*width, *height, false),
        Window::Hdr { width, height, .. } => (*width, *height, true),
    };
    let m = setting.margin;
    let size = (ww + 2 * m, wh + 2 * m);
    // SAFETY: a plain query.
    let max = unsafe { dc.GetMaximumBitmapSize() };
    if size.0 > max || size.1 > max || ww == 0 || wh == 0 {
        return Err(windows::Win32::Foundation::E_INVALIDARG.into());
    }
    // Linear light: half floats are plenty for SDR; HDR keeps its floats.
    let float = if hdr { DXGI_FORMAT_R32G32B32A32_FLOAT } else { DXGI_FORMAT_R16G16B16A16_FLOAT };
    let window_size = D2D_SIZE_U { width: ww, height: wh };
    // The window in linear light.
    let linear = match window {
        Window::Sdr { stride, bgra, .. } => {
            let props = props(DXGI_FORMAT_B8G8R8A8_UNORM, D2D1_ALPHA_MODE_IGNORE, D2D1_BITMAP_OPTIONS_NONE);
            // SAFETY: `bgra` holds `wh` rows `stride` bytes apart.
            let srgb = unsafe { dc.CreateBitmap(window_size, Some(bgra.as_ptr().cast()), *stride as u32, &props)? };
            let linear = target(&dc, (ww, wh), float)?;
            let image = convert(&dc, &srgb.cast()?, D2D1_COLOR_SPACE_SRGB, D2D1_COLOR_SPACE_SCRGB)?;
            draw_into(&dc, &linear, || {
                // SAFETY: plain drawing call.
                unsafe {
                    dc.DrawImage(
                        &image,
                        None,
                        None,
                        D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                        D2D1_COMPOSITE_MODE_SOURCE_COPY,
                    );
                }
                Ok(())
            })?;
            linear
        }
        Window::Hdr { rgba, .. } => {
            let props = props(float, D2D1_ALPHA_MODE_IGNORE, D2D1_BITMAP_OPTIONS_NONE);
            // SAFETY: `rgba` holds `wh` rows of `ww` pixels of 4 floats.
            unsafe { dc.CreateBitmap(window_size, Some(rgba.as_ptr().cast()), ww * 16, &props)? }
        }
    };
    // The blur behind: the window made small and darker, blurred, enlarged smoothly.
    let behind = match setting.behind {
        Behind::Colour(_) => None,
        Behind::Blur => {
            let (sw, sh) = setting.small;
            let small = target(&dc, (sw, sh), float)?;
            draw_into(&dc, &small, || {
                // SAFETY: plain drawing calls.
                unsafe {
                    dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
                    dc.DrawBitmap(
                        &linear,
                        Some(&rect(sw, sh)),
                        setting.darken,
                        D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                        None,
                        None,
                    );
                }
                Ok(())
            })?;
            let blur = blurred(&dc, &small.cast()?, rect(sw, sh), sigma(setting.small_radius))?;
            let scale = effect(&dc, &CLSID_D2D1Scale, &clamped(&dc, &blur, rect(sw, sh))?)?;
            set(
                &scale,
                D2D1_SCALE_PROP_SCALE.0,
                D2D1_PROPERTY_TYPE_VECTOR2,
                &floats(&[size.0 as f32 / sw as f32, size.1 as f32 / sh as f32]),
            )?;
            set(&scale, D2D1_SCALE_PROP_CENTER_POINT.0, D2D1_PROPERTY_TYPE_VECTOR2, &floats(&[0.0, 0.0]))?;
            set_enum(&scale, D2D1_SCALE_PROP_INTERPOLATION_MODE.0, D2D1_SCALE_INTERPOLATION_MODE_LINEAR.0)?;
            set_enum(&scale, D2D1_SCALE_PROP_BORDER_MODE.0, D2D1_BORDER_MODE_HARD.0)?;
            Some(output(&scale)?)
        }
    };
    // The window's shape; wider by `grow` and lower by `dy`, for the shadow.
    let window_rect = |dy: u32, grow: u32| D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: m as f32 - grow as f32,
            top: (m + dy) as f32 - grow as f32,
            right: (m + ww + grow) as f32,
            bottom: (m + dy + wh + grow) as f32,
        },
        radiusX: setting.radius + grow as f32,
        radiusY: setting.radius + grow as f32,
    };
    let shape = target(&dc, size, DXGI_FORMAT_B8G8R8A8_UNORM)?;
    draw_into(&dc, &shape, || {
        // SAFETY: plain drawing calls.
        unsafe {
            dc.Clear(None);
            let white = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
            dc.FillRoundedRectangle(&window_rect(setting.offset, setting.spread), &white);
        }
        Ok(())
    })?;
    let shadow = effect(&dc, &CLSID_D2D1Shadow, &shape.cast()?)?;
    set(
        &shadow,
        D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0,
        D2D1_PROPERTY_TYPE_FLOAT,
        &sigma(setting.softness).to_le_bytes(),
    )?;
    set(&shadow, D2D1_SHADOW_PROP_COLOR.0, D2D1_PROPERTY_TYPE_VECTOR4, &floats(&[0.0, 0.0, 0.0, setting.shadow]))?;
    set_enum(&shadow, D2D1_SHADOW_PROP_OPTIMIZATION.0, D2D1_SHADOW_OPTIMIZATION_QUALITY.0)?;
    let shadow = output(&shadow)?;
    // Everything together, in linear light.
    let composed = target(&dc, size, float)?;
    draw_into(&dc, &composed, || {
        // SAFETY: plain drawing calls; the brush draws the window's own pixels (whole-pixel offset, no smoothing).
        unsafe {
            match (&behind, setting.behind) {
                (Some(blur), _) => dc.DrawImage(
                    blur,
                    None,
                    Some(&rect(size.0, size.1)),
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_COPY,
                ),
                (None, Behind::Colour([r, g, b])) => {
                    dc.Clear(Some(&D2D1_COLOR_F { r, g, b, a: 1.0 }));
                }
                (None, Behind::Blur) => {}
            }
            dc.DrawImage(&shadow, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            let brush = dc.CreateBitmapBrush(
                &linear,
                Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                }),
                None,
            )?;
            brush.SetTransform(&Matrix3x2::translation(m as f32, m as f32));
            dc.FillRoundedRectangle(&window_rect(0, 0), &brush);
        }
        Ok(())
    })?;
    if hdr {
        let bytes = crate::gfx::read_back(&composed, size, 16)?;
        return Ok(Set::Hdr(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()));
    }
    let out = target(&dc, size, DXGI_FORMAT_B8G8R8A8_UNORM)?;
    let image = convert(&dc, &composed.cast()?, D2D1_COLOR_SPACE_SCRGB, D2D1_COLOR_SPACE_SRGB)?;
    draw_into(&dc, &out, || {
        // SAFETY: plain drawing call.
        unsafe {
            dc.DrawImage(&image, None, None, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_COMPOSITE_MODE_SOURCE_COPY);
        }
        Ok(())
    })?;
    Ok(Set::Sdr(crate::gfx::read_back(&out, size, 4)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::d3d_device;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;

    fn card(window: &Window<'_>, setting: &Setting) -> Result<Set> {
        set_on_card(Some(&d3d_device(D3D_DRIVER_TYPE_HARDWARE)?), window, setting)
    }
    use vixeeny_image::backdrop::{self as cpu, Backdrop, Linear};

    /// A window with detail: a gradient, a dark bar, a bright square.
    fn window(w: u32, h: u32) -> Vec<u8> {
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut px = [(x * 255 / w) as u8, (y * 255 / h) as u8, 180, 255];
                if (20..40).contains(&y) {
                    px = [30, 30, 30, 255];
                }
                if (x as i32 - 150).abs() < 25 && (y as i32 - 100).abs() < 25 {
                    px = [250, 250, 250, 255];
                }
                bgra.extend_from_slice(&px);
            }
        }
        bgra
    }

    fn setting(layout: &cpu::Layout, behind: Behind) -> Setting {
        Setting {
            margin: layout.margin,
            radius: layout.radius,
            offset: layout.offset,
            spread: layout.spread,
            softness: layout.softness,
            small: layout.small,
            small_radius: layout.small_radius,
            darken: cpu::DARKEN,
            shadow: cpu::SHADOW,
            behind,
        }
    }

    /// Mean difference and share of pixels off by more than 8 levels.
    fn compare(a: &[u8], b: &[u8]) -> (f64, f64) {
        assert_eq!(a.len(), b.len());
        let mut sum = 0_u64;
        let mut far = 0;
        for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
            let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
            sum += u64::from(d);
            far += usize::from(d > 8);
        }
        let n = (a.len() / 4) as f64;
        (sum as f64 / n, far as f64 / n)
    }

    #[test]
    fn the_card_sets_a_window_as_the_processor_does() {
        let (w, h, scale) = (320, 200, 1.5);
        let bgra = window(w, h);
        let layout = cpu::layout(w, h, scale);
        let linear = cpu::linear_from_bgra(w, h, w as usize * 4, &bgra);
        for (name, backdrop, behind, limits) in [
            (
                "colour",
                Backdrop::Colour([40, 90, 200]),
                Behind::Colour(cpu::linear_colour([40, 90, 200], 1.0)),
                (1.0, 0.01),
            ),
            ("blur", Backdrop::Blur, Behind::Blur, (2.5, 0.03)),
        ] {
            let sdr = Window::Sdr { width: w, height: h, stride: w as usize * 4, bgra: &bgra };
            let card = match card(&sdr, &setting(&layout, behind)) {
                Ok(Set::Sdr(card)) => card,
                Ok(Set::Hdr(_)) => panic!("an SDR window gives SDR"),
                Err(e) => {
                    vixeeny_common::skip_test(format!("no graphics card ({e})"));
                    return;
                }
            };
            let expected = cpu::linear_to_bgra(&cpu::set_on(&linear, &backdrop, scale, 1.0));
            let (sw, sh) = layout.size;
            crate::tests::save(&format!("backdrop-{name}-card"), sw, sh, &card);
            crate::tests::save(&format!("backdrop-{name}-cpu"), sw, sh, &expected);
            let (mean, far) = compare(&card, &expected);
            assert!(mean < limits.0 && far < limits.1, "{name}: mean {mean:.2}, far {:.2} %", far * 100.0);
            // The window itself is untouched.
            let centre = ((layout.margin + h / 2) * sw + layout.margin + w / 2) as usize * 4;
            let at = ((h / 2) * w + w / 2) as usize * 4;
            assert_eq!(card[centre..centre + 3], bgra[at..at + 3], "{name}");
        }
        // A gradient is only set by the processor: drawn to be looked at.
        let gradient = Backdrop::Gradient([0x65, 0x4e, 0xa3], [0xea, 0xaf, 0xc8]);
        let (sw, sh) = layout.size;
        crate::tests::save(
            "backdrop-gradient-cpu",
            sw,
            sh,
            &cpu::linear_to_bgra(&cpu::set_on(&linear, &gradient, scale, 1.0)),
        );
    }

    #[test]
    fn an_hdr_window_keeps_its_light() {
        let (w, h) = (160, 100);
        let rgba: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let v = (i % w) as f32 / w as f32 * 8.0;
                [v, v * 0.5, 0.2, 1.0]
            })
            .collect();
        let layout = cpu::layout(w, h, 1.0);
        let white = 3.0;
        let behind = Behind::Colour(cpu::linear_colour([255, 255, 255], white));
        let window = Window::Hdr { width: w, height: h, rgba: &rgba };
        let card = match card(&window, &setting(&layout, behind)) {
            Ok(Set::Hdr(card)) => card,
            Ok(Set::Sdr(_)) => panic!("an HDR window gives HDR"),
            Err(e) => {
                vixeeny_common::skip_test(format!("no graphics card ({e})"));
                return;
            }
        };
        let linear = Linear { width: w, height: h, rgba: rgba.clone() };
        let expected = cpu::set_on(&linear, &Backdrop::Colour([255, 255, 255]), 1.0, white);
        assert_eq!(card.len(), expected.rgba.len());
        // Far from the window: SDR white of the screen; in the window: its own light, beyond 1.
        assert!((card[0] - 3.0).abs() < 1e-3, "{}", card[0]);
        let (sw, m) = (layout.size.0, layout.margin);
        let at = |x: u32, y: u32| ((y + m) * sw + x + m) as usize * 4;
        let src = |x: u32, y: u32| ((y * w + x) * 4) as usize;
        assert!((card[at(150, 50)] - rgba[src(150, 50)]).abs() < 1e-3);
        let mean: f32 = card.iter().zip(&expected.rgba).map(|(a, b)| (a - b).abs()).sum::<f32>() / card.len() as f32;
        assert!(mean < 0.02, "mean {mean}");
    }
}
