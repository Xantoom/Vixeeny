// SPDX-License-Identifier: GPL-3.0-or-later
//! The GPU path of the recorder (plan 5.9): frames are converted and scaled by the D3D11 video
//! processor ([`crate::d3d_convert`]) into textures that FFmpeg's D3D11 frame pool owns, and the
//! hardware encoder reads those textures directly. Windows only; NVENC and AMF.
//!
//! Anything that fails while building the pipeline is an error the caller answers by using the
//! CPU path instead (there is a self-test conversion before the pipeline is handed out).

use ffmpeg_next::ffi;

use crate::recorder::RecordError;
use crate::registry::Encoder;

/// Encoders that take D3D11 frames as they are. (QSV wants its own surface type.)
pub fn supports(encoder: &Encoder) -> bool {
    (encoder.id.starts_with("nvenc_") || encoder.id.starts_with("amf_"))
        && encoder.hw_frames.iter().any(|f| f == "d3d11")
}

/// A frame whose pixels are in GPU memory (an FFmpeg `AVFrame` of format D3D11).
pub struct HwFrame(*mut ffi::AVFrame);

// SAFETY: an `AVFrame` is a reference-counted handle; the frame is only touched by one thread at
// a time (it travels through the recorder's queue).
unsafe impl Send for HwFrame {}

impl HwFrame {
    pub(crate) fn as_ptr(&self) -> *mut ffi::AVFrame {
        self.0
    }
}

impl Drop for HwFrame {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `av_frame_alloc` and is freed once.
        unsafe { ffi::av_frame_free(&raw mut self.0) };
    }
}

impl std::fmt::Debug for HwFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HwFrame")
    }
}

/// The converter and the pool of output textures.
pub struct GpuPipeline {
    frames: *mut ffi::AVBufferRef,
    device: *mut ffi::AVBufferRef,
    #[cfg(windows)]
    imp: windows_impl::Parts,
}

// SAFETY: the buffer references are immutable after creation (FFmpeg's pool is thread safe) and
// the D3D11 device is multithread protected.
unsafe impl Send for GpuPipeline {}
// SAFETY: see above.
unsafe impl Sync for GpuPipeline {}

impl std::fmt::Debug for GpuPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GpuPipeline")
    }
}

impl GpuPipeline {
    /// The frame pool, for the encoder's `hw_frames_ctx`.
    pub(crate) fn frames_ref(&self) -> *mut ffi::AVBufferRef {
        self.frames
    }
}

impl Drop for GpuPipeline {
    fn drop(&mut self) {
        // SAFETY: both references were created by this pipeline and are released once.
        unsafe {
            ffi::av_buffer_unref(&raw mut self.frames);
            ffi::av_buffer_unref(&raw mut self.device);
        }
    }
}

#[cfg(windows)]
pub use windows_impl::Source;

#[cfg(windows)]
mod windows_impl {
    use super::{GpuPipeline, HwFrame};
    use crate::clock::Fps;
    use crate::d3d_convert::{Conversion, VideoConverter};
    use crate::recorder::RecordError;
    use ffmpeg_next::ffi;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
        D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
    };
    use windows::core::Interface;

    pub struct Parts {
        converter: VideoConverter,
        context: ID3D11DeviceContext,
    }

    /// What the capture delivers.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Source {
        pub size: (u32, u32),
        /// scRGB half floats (HDR monitor) instead of BGRA8.
        pub hdr: bool,
    }

    fn cfg_err(what: &str, detail: impl std::fmt::Display) -> RecordError {
        RecordError::Config(format!("GPU path: {what}: {detail}"))
    }

    impl GpuPipeline {
        /// Builds the video processor, the FFmpeg D3D11 device and frame pool on `device`, and
        /// converts one blank frame as a self-test. `ten_bit`: P010 instead of NV12 (always for
        /// an HDR output).
        pub fn new(
            device: &ID3D11Device,
            context: &ID3D11DeviceContext,
            source: Source,
            out: (u32, u32),
            fps: Fps,
            hdr_output: bool,
            ten_bit: bool,
        ) -> Result<Self, RecordError> {
            let converter = VideoConverter::new(
                device,
                context,
                source.size,
                out,
                (fps.num, fps.den),
                Conversion {
                    hdr_source: source.hdr,
                    hdr_output,
                },
            )
            .map_err(|e| cfg_err("video processor", e))?;

            // SAFETY: plain FFmpeg allocation and initialisation calls; every return value is
            // checked and partial state is released on failure.
            let (hw_device, frames) = unsafe {
                let mut hw_device =
                    ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
                if hw_device.is_null() {
                    return Err(cfg_err("D3D11VA device", "allocation failed"));
                }
                let dctx = (*hw_device).data.cast::<ffi::AVHWDeviceContext>();
                let d3d = (*dctx).hwctx.cast::<ffi::AVD3D11VADeviceContext>();
                // FFmpeg releases the device when the context is freed: give it its own reference.
                (*d3d).device = device.clone().into_raw().cast();
                if ffi::av_hwdevice_ctx_init(hw_device) < 0 {
                    ffi::av_buffer_unref(&raw mut hw_device);
                    return Err(cfg_err("D3D11VA device", "init failed"));
                }
                let mut frames = ffi::av_hwframe_ctx_alloc(hw_device);
                if frames.is_null() {
                    ffi::av_buffer_unref(&raw mut hw_device);
                    return Err(cfg_err("frame pool", "allocation failed"));
                }
                let fctx = (*frames).data.cast::<ffi::AVHWFramesContext>();
                (*fctx).format = ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
                (*fctx).sw_format = if ten_bit {
                    ffi::AVPixelFormat::AV_PIX_FMT_P010LE
                } else {
                    ffi::AVPixelFormat::AV_PIX_FMT_NV12
                };
                (*fctx).width = out.0 as i32;
                (*fctx).height = out.1 as i32;
                // Enough for the encoder's own queue, within about 300 MB of video memory.
                let bytes =
                    u64::from(out.0) * u64::from(out.1) * 3 / 2 * if ten_bit { 2 } else { 1 };
                (*fctx).initial_pool_size = (300_000_000 / bytes.max(1)).clamp(8, 20) as i32;
                let hwctx = (*fctx).hwctx.cast::<ffi::AVD3D11VAFramesContext>();
                (*hwctx).BindFlags =
                    (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
                if ffi::av_hwframe_ctx_init(frames) < 0 {
                    ffi::av_buffer_unref(&raw mut frames);
                    ffi::av_buffer_unref(&raw mut hw_device);
                    return Err(cfg_err("frame pool", "init failed (format not supported?)"));
                }
                (hw_device, frames)
            };
            let pipeline = Self {
                frames,
                device: hw_device,
                imp: Parts {
                    converter,
                    context: context.clone(),
                },
            };

            // Self-test: one blank source texture through the whole conversion.
            let desc = D3D11_TEXTURE2D_DESC {
                Width: source.size.0,
                Height: source.size.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: if source.hdr {
                    DXGI_FORMAT_R16G16B16A16_FLOAT
                } else {
                    DXGI_FORMAT_B8G8R8A8_UNORM
                },
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let mut blank = None;
            // SAFETY: `desc` is a valid description; the out-pointer is valid.
            unsafe { device.CreateTexture2D(&raw const desc, None, Some(&raw mut blank)) }
                .map_err(|e| cfg_err("test texture", e))?;
            let blank = blank.ok_or_else(|| cfg_err("test texture", "none created"))?;
            let rect = RECT {
                left: 0,
                top: 0,
                right: source.size.0 as i32,
                bottom: source.size.1 as i32,
            };
            pipeline.convert(&blank, rect)?;
            Ok(pipeline)
        }

        /// Converts the `rect` part of `texture` into a fresh pool frame. `Err` when the pool is
        /// exhausted (the encoder is behind: the caller drops the frame) or the GPU refuses.
        pub fn convert(
            &self,
            texture: &ID3D11Texture2D,
            rect: RECT,
        ) -> Result<HwFrame, RecordError> {
            // SAFETY: the frame is allocated from our own pool; its `data[0]` is the pool
            // texture and `data[1]` the array slice, as FFmpeg documents for D3D11 frames.
            unsafe {
                let mut frame = ffi::av_frame_alloc();
                if frame.is_null() {
                    return Err(cfg_err("frame", "allocation failed"));
                }
                if ffi::av_hwframe_get_buffer(self.frames_ref(), frame, 0) < 0 {
                    ffi::av_frame_free(&raw mut frame);
                    return Err(cfg_err("frame pool", "exhausted"));
                }
                let hw = HwFrame(frame);
                let dst_ptr = (*frame).data[0].cast::<std::ffi::c_void>();
                let slice = (*frame).data[1] as usize as u32;
                let dst = ID3D11Texture2D::from_raw_borrowed(&dst_ptr)
                    .ok_or_else(|| cfg_err("frame texture", "missing"))?;
                self.imp
                    .converter
                    .convert(texture, rect, dst, slice)
                    .map_err(|e| cfg_err("conversion", e))?;
                // The encoder reads the texture on its own thread: make the GPU work visible.
                self.imp.context.Flush();
                Ok(hw)
            }
        }
    }
}

/// Opens `ctx` for D3D11 frames of this pipeline.
pub(crate) fn attach(
    ctx: &mut ffmpeg_next::encoder::video::Video,
    pipeline: &GpuPipeline,
) -> Result<(), RecordError> {
    // SAFETY: `ctx` is a live, not yet opened codec context; `av_buffer_ref` gives it its own
    // reference to the pool, which it releases when closed.
    unsafe {
        let raw = ctx.as_mut_ptr();
        (*raw).pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
        (*raw).hw_frames_ctx = ffi::av_buffer_ref(pipeline.frames_ref());
        if (*raw).hw_frames_ctx.is_null() {
            return Err(RecordError::Config(
                "GPU path: cannot reference the pool".into(),
            ));
        }
    }
    Ok(())
}
