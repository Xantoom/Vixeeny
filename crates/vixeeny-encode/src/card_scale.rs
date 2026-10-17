// SPDX-License-Identifier: GPL-3.0-or-later
//! The frames the graphics card decodes, made ready for the preview on the card itself: the
//! video processor scales them to the size shown and turns them into RGB, so that only that
//! small picture crosses to memory (rather than the whole frame, scaled and converted on the
//! processor). An HDR frame comes back as 10-bit PQ RGB, which the preview's tone mapping reads.

use ffmpeg_next::ffi;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
    DXGI_COLOR_SPACE_TYPE, DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
    DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020,
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use crate::cut::ToneMap;
use crate::d3d_convert::VideoConverter;

/// What the converter was made for: the frame's size, the size shown, its colour space.
type Made = ((u32, u32), (u32, u32), DXGI_COLOR_SPACE_TYPE);

pub struct CardScaler {
    converter: Option<(Made, VideoConverter, ID3D11Texture2D, ID3D11Texture2D)>,
}

fn err(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

/// The colour space of a decoded frame (what the video processor reads).
fn space_of(frame: &ffi::AVFrame, hdr: bool) -> DXGI_COLOR_SPACE_TYPE {
    if hdr {
        return DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020;
    }
    let full = frame.color_range == ffi::AVColorRange::AVCOL_RANGE_JPEG;
    let sd = matches!(
        frame.colorspace,
        ffi::AVColorSpace::AVCOL_SPC_BT470BG | ffi::AVColorSpace::AVCOL_SPC_SMPTE170M
    );
    match (full, sd) {
        (false, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
        (false, true) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
        (true, false) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
        (true, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
    }
}

fn texture(
    device: &ID3D11Device,
    (w, h): (u32, u32),
    format: DXGI_FORMAT,
    staging: bool,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: if staging {
            0
        } else {
            D3D11_BIND_RENDER_TARGET.0 as u32
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut out = None;
    // SAFETY: `desc` describes a plain texture; the out-pointer is valid.
    unsafe { device.CreateTexture2D(&desc, None, Some(&raw mut out)) }
        .map_err(|e| err("CreateTexture2D", e))?;
    out.ok_or_else(|| "no texture".into())
}

/// Holds FFmpeg's lock of the device context while alive (the decoder uses it too).
struct Locked<'a>(&'a ffi::AVD3D11VADeviceContext);

impl<'a> Locked<'a> {
    fn new(d3d: &'a ffi::AVD3D11VADeviceContext) -> Self {
        if let Some(lock) = d3d.lock {
            // SAFETY: FFmpeg's lock of this device, with its own context pointer.
            unsafe { lock(d3d.lock_ctx) };
        }
        Self(d3d)
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        if let Some(unlock) = self.0.unlock {
            // SAFETY: matches the lock taken in `new`.
            unsafe { unlock(self.0.lock_ctx) };
        }
    }
}

impl CardScaler {
    pub fn new() -> Self {
        Self { converter: None }
    }

    /// `frame` (decoded by the card) as `size` BGRA in `bgra`; an HDR frame is tone mapped
    /// with `tone`.
    ///
    /// # Safety
    /// `frame` is a live D3D11 frame of FFmpeg (`data[0]` its texture, `data[1]` the slice,
    /// `hw_frames_ctx` set).
    pub unsafe fn run(
        &mut self,
        frame: *const ffi::AVFrame,
        size: (u32, u32),
        bgra: &mut [u8],
        tone: Option<&ToneMap>,
    ) -> Result<(), String> {
        // SAFETY: per the contract, the frame and its contexts are FFmpeg's, alive for the
        // call; the device context's lock is held while it is used.
        unsafe {
            let f = &*frame;
            let frames = (*f.hw_frames_ctx).data.cast::<ffi::AVHWFramesContext>();
            let device_ctx = (*frames).device_ctx;
            let d3d = &*(*device_ctx).hwctx.cast::<ffi::AVD3D11VADeviceContext>();
            let device = ID3D11Device::from_raw_borrowed(&d3d.device.cast())
                .ok_or("no device")?
                .clone();
            let context = ID3D11DeviceContext::from_raw_borrowed(&d3d.device_context.cast())
                .ok_or("no device context")?
                .clone();
            let source = ID3D11Texture2D::from_raw_borrowed(&f.data[0].cast())
                .ok_or("no texture")?
                .clone();
            let slice = f.data[1] as usize as u32;
            let src = (f.width.max(1) as u32, f.height.max(1) as u32);
            let hdr = tone.is_some();
            let made = (src, size, space_of(f, hdr));
            let _locked = Locked::new(d3d);
            if self.converter.as_ref().is_none_or(|(m, ..)| *m != made) {
                self.converter = None;
                let (format, out_space) = if hdr {
                    (
                        DXGI_FORMAT_R10G10B10A2_UNORM,
                        DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
                    )
                } else {
                    (
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                    )
                };
                let converter = VideoConverter::with_spaces(
                    &device,
                    &context,
                    src,
                    size,
                    (60, 1),
                    (made.2, out_space),
                )?;
                let target = texture(&device, size, format, false)?;
                let staging = texture(&device, size, format, true)?;
                self.converter = Some((made, converter, target, staging));
            }
            let Some((_, converter, target, staging)) = &self.converter else {
                return Err("no converter".into());
            };
            let whole = RECT {
                left: 0,
                top: 0,
                right: src.0 as i32,
                bottom: src.1 as i32,
            };
            converter.convert_slice(&source, slice, whole, target, 0)?;
            context.CopyResource(staging, target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
                .map_err(|e| err("Map", e))?;
            let (w, h) = (size.0 as usize, size.1 as usize);
            let stride = mapped.RowPitch as usize;
            let rows = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * h);
            match tone {
                Some(tone) => tone.run_with::<4>(rows, stride, (w, h), bgra, |px| {
                    let v = u32::from_le_bytes(*px);
                    // 10-bit codes, made 16-bit.
                    let c = |shift: u32| (((v >> shift) & 0x3ff) << 6) as u16;
                    [c(0), c(10), c(20)]
                }),
                None => {
                    for (row, out) in bgra.chunks_exact_mut(w * 4).enumerate().take(h) {
                        out.copy_from_slice(&rows[row * stride..][..w * 4]);
                    }
                }
            }
            context.Unmap(staging, 0);
            Ok(())
        }
    }
}
