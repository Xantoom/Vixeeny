// SPDX-License-Identifier: GPL-3.0-or-later
//! The GPU path of the recorder (plan 5.9): frames are converted and scaled by the D3D11 video
//! processor ([`crate::d3d_convert`]) into textures that FFmpeg's D3D11 frame pool owns. NVENC
//! and AMF read those textures directly, QSV through a QSV view of the same pool, and software
//! encoders get them downloaded (NV12 / P010: well under half the bytes of the captured BGRA,
//! and no colour conversion left for the CPU). Windows only.
//!
//! Anything that fails while building the pipeline is an error the caller answers by using the
//! CPU path instead (there is a self-test conversion before the pipeline is handed out).

use ffmpeg_next::ffi;

use crate::recorder::RecordError;
use crate::registry::{Encoder, Kind};

/// How an encoder takes the frames of the GPU path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// D3D11 textures as they are (NVENC, AMF).
    Direct,
    /// Mapped to QSV surfaces, without a copy.
    Qsv,
    /// Downloaded to system memory (software encoders).
    Download,
}

/// How `encoder` can take GPU frames; `None` when it cannot.
pub fn feed(encoder: &Encoder) -> Option<Feed> {
    let d3d11 = encoder.hw_frames.iter().any(|f| f == "d3d11");
    if encoder.kind == Kind::Software {
        Some(Feed::Download)
    } else if d3d11 && (encoder.id.starts_with("nvenc_") || encoder.id.starts_with("amf_")) {
        Some(Feed::Direct)
    } else if d3d11 && encoder.id.starts_with("qsv_") {
        Some(Feed::Qsv)
    } else {
        None
    }
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
    feed: Feed,
    /// [`Feed::Qsv`]: the QSV frame context derived from `frames`.
    qsv: *mut ffi::AVBufferRef,
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

    pub fn feed(&self) -> Feed {
        self.feed
    }

    /// [`Feed::Qsv`]: `frame` seen as a QSV surface (the same texture, no copy).
    pub(crate) fn to_qsv(&self, frame: &HwFrame) -> Result<HwFrame, RecordError> {
        // SAFETY: `self.qsv` was derived from the pool `frame` comes from; the mapped frame
        // keeps a reference to the source frame until it is freed.
        unsafe {
            let mut out = ffi::av_frame_alloc();
            if out.is_null() {
                return Err(RecordError::Config(
                    "GPU path: frame allocation failed".into(),
                ));
            }
            (*out).format = ffi::AVPixelFormat::AV_PIX_FMT_QSV as i32;
            (*out).hw_frames_ctx = ffi::av_buffer_ref(self.qsv);
            let code = ffi::av_hwframe_map(out, frame.as_ptr(), ffi::AV_HWFRAME_MAP_READ as i32);
            if code < 0 {
                ffi::av_frame_free(&raw mut out);
                return Err(RecordError::Ffmpeg(format!("QSV map: {code}")));
            }
            Ok(HwFrame(out))
        }
    }

    /// [`Feed::Download`]: the pixels of `frame` in system memory (NV12 or P010).
    pub(crate) fn download(
        &self,
        frame: &HwFrame,
    ) -> Result<ffmpeg_next::frame::Video, RecordError> {
        let mut out = ffmpeg_next::frame::Video::empty();
        // SAFETY: `out` is an empty frame: FFmpeg allocates it in the pool's software format.
        let code = unsafe { ffi::av_hwframe_transfer_data(out.as_mut_ptr(), frame.as_ptr(), 0) };
        if code < 0 {
            return Err(RecordError::Ffmpeg(format!("GPU download: {code}")));
        }
        Ok(out)
    }
}

impl Drop for GpuPipeline {
    fn drop(&mut self) {
        // SAFETY: the references were created by this pipeline and are released once (unref of
        // a null reference is a no-op).
        unsafe {
            ffi::av_buffer_unref(&raw mut self.qsv);
            ffi::av_buffer_unref(&raw mut self.frames);
            ffi::av_buffer_unref(&raw mut self.device);
        }
    }
}

#[cfg(windows)]
pub use windows_impl::Source;

#[cfg(windows)]
mod windows_impl {
    use super::{Feed, GpuPipeline, HwFrame};
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
        /// Builds the video processor, the FFmpeg D3D11 device and frame pool on `device` (and
        /// their QSV view for [`Feed::Qsv`]), and converts one blank frame as a self-test.
        /// `ten_bit`: P010 instead of NV12 (always for an HDR output).
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            device: &ID3D11Device,
            context: &ID3D11DeviceContext,
            source: Source,
            out: (u32, u32),
            fps: Fps,
            hdr_output: bool,
            ten_bit: bool,
            feed: Feed,
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
                // NVIDIA refuses NV12 / P010 texture arrays that can be render targets (what the
                // video processor writes to): a pool of single textures, which FFmpeg grows on
                // demand and reuses. QSV needs a fixed array to derive its surfaces from:
                // enough for the encoder's own queue, within about 300 MB of video memory.
                let bytes =
                    u64::from(out.0) * u64::from(out.1) * 3 / 2 * if ten_bit { 2 } else { 1 };
                (*fctx).initial_pool_size = if feed == Feed::Qsv {
                    (300_000_000 / bytes.max(1)).clamp(8, 20) as i32
                } else {
                    0
                };
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
            let qsv = if feed == Feed::Qsv {
                // SAFETY: derivation from the live device and pool; the QSV device is only
                // held by the derived frame context.
                unsafe {
                    let mut qsv_device = std::ptr::null_mut();
                    let mut qsv = std::ptr::null_mut();
                    let code = if ffi::av_hwdevice_ctx_create_derived(
                        &raw mut qsv_device,
                        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_QSV,
                        hw_device,
                        0,
                    ) < 0
                    {
                        -1
                    } else {
                        ffi::av_hwframe_ctx_create_derived(
                            &raw mut qsv,
                            ffi::AVPixelFormat::AV_PIX_FMT_QSV,
                            qsv_device,
                            frames,
                            ffi::AV_HWFRAME_MAP_DIRECT as i32,
                        )
                    };
                    ffi::av_buffer_unref(&raw mut qsv_device);
                    if code < 0 {
                        let (mut frames, mut hw_device) = (frames, hw_device);
                        ffi::av_buffer_unref(&raw mut frames);
                        ffi::av_buffer_unref(&raw mut hw_device);
                        return Err(cfg_err("QSV view of the pool", code));
                    }
                    qsv
                }
            } else {
                std::ptr::null_mut()
            };
            let pipeline = Self {
                frames,
                device: hw_device,
                feed,
                qsv,
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
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
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
            let test = pipeline.convert(&blank, rect)?;
            match feed {
                Feed::Direct => {}
                Feed::Qsv => drop(pipeline.to_qsv(&test)?),
                Feed::Download => drop(pipeline.download(&test)?),
            }
            Ok(pipeline)
        }

        /// Converts the `rect` part of `texture` into a pool frame. `Err` when the pool is
        /// exhausted (QSV's fixed pool: the encoder is behind, the caller drops the frame) or
        /// the GPU refuses.
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

/// Opens `ctx` for the frames of this pipeline: D3D11 or QSV surfaces. A software encoder keeps
/// its own pixel format (the frames are downloaded).
pub(crate) fn attach(
    ctx: &mut ffmpeg_next::encoder::video::Video,
    pipeline: &GpuPipeline,
) -> Result<(), RecordError> {
    let (format, pool) = match pipeline.feed {
        Feed::Direct => (ffi::AVPixelFormat::AV_PIX_FMT_D3D11, pipeline.frames),
        Feed::Qsv => (ffi::AVPixelFormat::AV_PIX_FMT_QSV, pipeline.qsv),
        Feed::Download => return Ok(()),
    };
    // SAFETY: `ctx` is a live, not yet opened codec context; `av_buffer_ref` gives it its own
    // reference to the pool, which it releases when closed.
    unsafe {
        let raw = ctx.as_mut_ptr();
        (*raw).pix_fmt = format;
        (*raw).hw_frames_ctx = ffi::av_buffer_ref(pool);
        if (*raw).hw_frames_ctx.is_null() {
            return Err(RecordError::Config(
                "GPU path: cannot reference the pool".into(),
            ));
        }
    }
    Ok(())
}
