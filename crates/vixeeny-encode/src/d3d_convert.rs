// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU colour conversion and scaling (Windows): a D3D11 video processor turns the
//! captured BGRA8 (or scRGB half-float, HDR) texture into NV12 / P010 in another texture, with
//! the BT.709 or BT.2020/PQ matrix. Nothing crosses the PCIe bus: the output texture goes to the
//! hardware encoder as it is (see `gpu`).

use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11_TEX2D_ARRAY_VPOV, D3D11_TEX2D_VPIV, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2DARRAY, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext1,
    ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
    DXGI_COLOR_SPACE_TYPE, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020, DXGI_FORMAT, DXGI_RATIONAL,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
};
use windows::core::Interface;

/// The types the GPU path exchanges, for callers that do not depend on `windows` themselves.
pub use windows::Win32::Foundation::RECT as Rect;
pub use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device as Device, ID3D11DeviceContext as DeviceContext, ID3D11Texture2D as Texture,
};

/// What the converter produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conversion {
    /// The source is scRGB half floats (an HDR monitor), not BGRA8.
    pub hdr_source: bool,
    /// Rec. 2020 / PQ output (implies 10-bit); else Rec. 709.
    pub hdr_output: bool,
}

pub struct VideoConverter {
    video_context: ID3D11VideoContext1,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    video_device: ID3D11VideoDevice,
    out: (u32, u32),
    /// The views of the textures already met: a capture and an encoder go round the same few,
    /// so each view is made once rather than at every frame.
    views: std::sync::Mutex<Views>,
}

/// Views by texture (its address: the view holds the texture, so no other takes the address
/// while it is kept) and array slice.
#[derive(Default)]
struct Views {
    inputs: Vec<((usize, u32), ID3D11VideoProcessorInputView)>,
    outputs: Vec<((usize, u32), ID3D11VideoProcessorOutputView)>,
}

/// Input views kept: the two textures of a capture pool. No more: a view holds its texture, and
/// a window resized gets new ones (the old ones must go, they are large).
const KEPT_INPUTS: usize = 2;
/// Output views kept: the slices of the encoder's frames, one texture.
const KEPT_OUTPUTS: usize = 32;

/// The view of `key` in `views`, made by `make` the first time (the oldest goes past `kept`).
fn view<V: Clone>(
    views: &mut Vec<((usize, u32), V)>,
    kept: usize,
    key: (usize, u32),
    make: impl FnOnce() -> Result<V, String>,
) -> Result<V, String> {
    if let Some((_, v)) = views.iter().find(|(k, _)| *k == key) {
        return Ok(v.clone());
    }
    let v = make()?;
    if views.len() >= kept {
        views.remove(0);
    }
    views.push((key, v.clone()));
    Ok(v)
}

fn err(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

/// A D3D11 device on the adapter of `vendor_id` (the encoder's GPU) or the default adapter, with
/// video support and multithread protection (capture and encoder threads share it).
pub fn create_device(
    vendor_id: Option<u32>,
) -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let adapter: Option<IDXGIAdapter> = vendor_id.and_then(|vendor| {
        // SAFETY: plain DXGI enumeration; every call is checked.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
        (0..)
            // SAFETY: `factory` is live; the index runs until the enumeration reports the end.
            .map_while(|i| unsafe { factory.EnumAdapters1(i) }.ok())
            .find(|a: &IDXGIAdapter1| {
                // SAFETY: `a` is a live adapter.
                unsafe { a.GetDesc1() }.is_ok_and(|d| d.VendorId == vendor)
            })
            .and_then(|a| a.cast().ok())
    });
    let mut device = None;
    let mut context = None;
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
    // SAFETY: out-pointers are valid; the adapter (when given) outlives the call.
    unsafe {
        D3D11CreateDevice(
            adapter.as_ref(),
            if adapter.is_some() {
                D3D_DRIVER_TYPE_UNKNOWN
            } else {
                D3D_DRIVER_TYPE_HARDWARE
            },
            HMODULE::default(),
            flags,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .map_err(|e| err("D3D11CreateDevice", e))?;
    let device = device.ok_or("no D3D11 device")?;
    let context = context.ok_or("no D3D11 context")?;
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        // SAFETY: a live interface of the device.
        let _ = unsafe { mt.SetMultithreadProtected(true) };
    }
    Ok((device, context))
}

impl VideoConverter {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        src: (u32, u32),
        out: (u32, u32),
        fps: (u32, u32),
        conversion: Conversion,
    ) -> Result<Self, String> {
        let spaces = match (conversion.hdr_source, conversion.hdr_output) {
            (true, true) => (
                DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020,
            ),
            (true, false) => (
                DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            ),
            (false, _) => (
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            ),
        };
        Self::with_spaces(device, context, src, out, fps, spaces)
    }

    /// A converter from `spaces.0` to `spaces.1` (any formats the video processor takes).
    pub fn with_spaces(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        src: (u32, u32),
        out: (u32, u32),
        fps: (u32, u32),
        (input_space, output_space): (DXGI_COLOR_SPACE_TYPE, DXGI_COLOR_SPACE_TYPE),
    ) -> Result<Self, String> {
        let video_device: ID3D11VideoDevice = device.cast().map_err(|e| err("video device", e))?;
        let video_context: ID3D11VideoContext1 =
            context.cast().map_err(|e| err("video context", e))?;
        let rate = DXGI_RATIONAL {
            Numerator: fps.0,
            Denominator: fps.1.max(1),
        };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: src.0,
            InputHeight: src.1,
            OutputFrameRate: rate,
            OutputWidth: out.0,
            OutputHeight: out.1,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        // SAFETY: `desc` is a valid descriptor; the interfaces are live.
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&raw const desc) }
            .map_err(|e| err("CreateVideoProcessorEnumerator", e))?;
        // SAFETY: as above.
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|e| err("CreateVideoProcessor", e))?;
        // SAFETY: the processor belongs to this video context; no pointers are involved.
        unsafe {
            video_context.VideoProcessorSetStreamColorSpace1(&processor, 0, input_space);
            video_context.VideoProcessorSetOutputColorSpace1(&processor, output_space);
            // No enhancement, no automatic processing: a faithful copy.
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
        }
        Ok(Self {
            video_context,
            enumerator,
            processor,
            video_device,
            out,
            views: std::sync::Mutex::default(),
        })
    }

    /// Whether the video processor can write `format` (drivers differ beyond NV12 / P010).
    pub fn writes(&self, format: DXGI_FORMAT) -> bool {
        // SAFETY: a plain query of a live enumerator.
        unsafe { self.enumerator.CheckVideoProcessorFormat(format) }.is_ok_and(|support| {
            support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 != 0
        })
    }

    /// Converts the `src_rect` part of `src` into slice `dst_slice` of `dst` (NV12 or P010, the
    /// output size given at creation: the part is scaled to fill it).
    pub fn convert(
        &self,
        src: &ID3D11Texture2D,
        src_rect: RECT,
        dst: &ID3D11Texture2D,
        dst_slice: u32,
    ) -> Result<(), String> {
        self.convert_slice(src, 0, src_rect, dst, dst_slice)
    }

    /// Like [`convert`](Self::convert), from slice `src_slice` of a texture array (the frames
    /// of a decoder).
    pub fn convert_slice(
        &self,
        src: &ID3D11Texture2D,
        src_slice: u32,
        src_rect: RECT,
        dst: &ID3D11Texture2D,
        dst_slice: u32,
    ) -> Result<(), String> {
        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: src_slice,
                },
            },
        };
        let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2DArray: D3D11_TEX2D_ARRAY_VPOV {
                    MipSlice: 0,
                    FirstArraySlice: dst_slice,
                    ArraySize: 1,
                },
            },
        };
        let mut views = self
            .views
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let input = view(
            &mut views.inputs,
            KEPT_INPUTS,
            (src.as_raw() as usize, src_slice),
            || {
                let mut input = None;
                // SAFETY: the descriptor and out-pointer are valid for the call; the texture
                // belongs to the device the converter was created on.
                unsafe {
                    self.video_device.CreateVideoProcessorInputView(
                        src,
                        &self.enumerator,
                        &raw const input_desc,
                        Some(&raw mut input),
                    )
                }
                .map_err(|e| err("input view", e))?;
                input.ok_or_else(|| "no input view".to_owned())
            },
        )?;
        let output = view(
            &mut views.outputs,
            KEPT_OUTPUTS,
            (dst.as_raw() as usize, dst_slice),
            || {
                let mut output = None;
                // SAFETY: as above.
                unsafe {
                    self.video_device.CreateVideoProcessorOutputView(
                        dst,
                        &self.enumerator,
                        &raw const output_desc,
                        Some(&raw mut output),
                    )
                }
                .map_err(|e| err("output view", e))?;
                output.ok_or_else(|| "no output view".to_owned())
            },
        )?;
        drop(views);
        let target = RECT {
            left: 0,
            top: 0,
            right: self.out.0 as i32,
            bottom: self.out.1 as i32,
        };
        // SAFETY: the views belong to this converter's device and enumerator.
        unsafe {
            self.video_context.VideoProcessorSetStreamSourceRect(
                &self.processor,
                0,
                true,
                Some(&raw const src_rect),
            );
            self.video_context.VideoProcessorSetStreamDestRect(
                &self.processor,
                0,
                true,
                Some(&raw const target),
            );
            self.video_context.VideoProcessorSetOutputTargetRect(
                &self.processor,
                true,
                Some(&raw const target),
            );
            let mut streams = [D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: std::mem::ManuallyDrop::new(Some(input)),
                ..Default::default()
            }];
            let result =
                self.video_context
                    .VideoProcessorBlt(&self.processor, &output, 0, &streams);
            // The struct does not release the view it carries: do it here.
            std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface);
            result.map_err(|e| err("VideoProcessorBlt", e))
        }
    }
}
