// SPDX-License-Identifier: GPL-3.0-or-later
//! VAAPI and Vulkan encoders (Linux) only take frames that live in GPU memory. This is the CPU
//! path to them: the recorder converts a frame to the encoder's software format (NV12, P010…)
//! as usual, and [`Upload`] copies it into a surface of the device's frame pool.
//!
//! The zero-copy path (DMA-BUF frames from the capture straight into the encoder) is a later
//! step; this one costs a copy per frame but works with every capture backend.

use ffmpeg_next::format::Pixel;
use ffmpeg_next::{ffi, frame};

use crate::gpu::HwFrame;
use crate::registry::Vendor;

/// A hardware device and the pool of surfaces frames are uploaded to.
pub struct Upload {
    device: *mut ffi::AVBufferRef,
    frames: *mut ffi::AVBufferRef,
    hw_format: ffi::AVPixelFormat,
}

// SAFETY: the buffer references are immutable after creation and FFmpeg's pools are thread safe;
// an `Upload` is used by the recording thread only.
unsafe impl Send for Upload {}

impl std::fmt::Debug for Upload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Upload")
    }
}

/// Whether `vendor` is an encoder family that needs [`Upload`].
pub fn needed(vendor: Vendor) -> bool {
    matches!(vendor, Vendor::Vaapi | Vendor::Vulkan)
}

impl Upload {
    /// A device of the family `vendor` (`device`: a render node such as `/dev/dri/renderD128`,
    /// or the default one) and a pool of `size` surfaces holding `sw_format` pixels.
    pub fn new(
        vendor: Vendor,
        sw_format: Pixel,
        size: (u32, u32),
        device: Option<&str>,
    ) -> Result<Self, String> {
        let (kind, hw_format) = match vendor {
            Vendor::Vaapi => (
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                ffi::AVPixelFormat::AV_PIX_FMT_VAAPI,
            ),
            Vendor::Vulkan => (
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
                ffi::AVPixelFormat::AV_PIX_FMT_VULKAN,
            ),
            _ => return Err("not a VAAPI or Vulkan encoder".into()),
        };
        let path = device
            .map(std::ffi::CString::new)
            .transpose()
            .map_err(|e| e.to_string())?;
        let mut device_ref: *mut ffi::AVBufferRef = std::ptr::null_mut();
        // SAFETY: `device_ref` receives a new reference on success; the path is a valid C string
        // or null (the default device); no options, no flags.
        let code = unsafe {
            ffi::av_hwdevice_ctx_create(
                &raw mut device_ref,
                kind,
                path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                std::ptr::null_mut(),
                0,
            )
        };
        if code < 0 {
            return Err(format!("no {vendor:?} device (error {code})"));
        }
        // SAFETY: `device_ref` is the live device reference created above; the frames context
        // is allocated from it, configured before `av_hwframe_ctx_init`, and released with it.
        let frames = unsafe {
            let mut frames = ffi::av_hwframe_ctx_alloc(device_ref);
            if frames.is_null() {
                ffi::av_buffer_unref(&raw mut device_ref);
                return Err("cannot allocate the surface pool".into());
            }
            let ctx = (*frames).data.cast::<ffi::AVHWFramesContext>();
            (*ctx).format = hw_format;
            (*ctx).sw_format = sw_format.into();
            (*ctx).width = size.0 as i32;
            (*ctx).height = size.1 as i32;
            (*ctx).initial_pool_size = 8;
            let code = ffi::av_hwframe_ctx_init(frames);
            if code < 0 {
                ffi::av_buffer_unref(&raw mut frames);
                ffi::av_buffer_unref(&raw mut device_ref);
                return Err(format!("cannot create the surface pool (error {code})"));
            }
            frames
        };
        Ok(Self {
            device: device_ref,
            frames,
            hw_format,
        })
    }

    /// Opens `ctx` for the surfaces of this pool.
    pub(crate) fn attach(
        &self,
        ctx: &mut ffmpeg_next::encoder::video::Video,
    ) -> Result<(), String> {
        // SAFETY: `ctx` is a live, not yet opened codec context; it takes its own reference to
        // the pool and releases it when closed.
        unsafe {
            let raw = ctx.as_mut_ptr();
            (*raw).pix_fmt = self.hw_format;
            (*raw).hw_frames_ctx = ffi::av_buffer_ref(self.frames);
            if (*raw).hw_frames_ctx.is_null() {
                return Err("cannot reference the surface pool".into());
            }
        }
        Ok(())
    }

    /// Copies a software frame into a new surface; timestamps and colour properties follow.
    pub fn upload(&self, software: &frame::Video) -> Result<HwFrame, String> {
        // SAFETY: the new frame is filled from the pool, then freed on every failure path;
        // `software` is a live frame whose properties are only read.
        unsafe {
            let mut hw = ffi::av_frame_alloc();
            if hw.is_null() {
                return Err("out of memory".into());
            }
            let mut code = ffi::av_hwframe_get_buffer(self.frames, hw, 0);
            if code >= 0 {
                code = ffi::av_hwframe_transfer_data(hw, software.as_ptr(), 0);
            }
            if code >= 0 {
                code = ffi::av_frame_copy_props(hw, software.as_ptr());
            }
            if code < 0 {
                ffi::av_frame_free(&raw mut hw);
                return Err(format!("cannot upload the frame (error {code})"));
            }
            Ok(HwFrame::from_raw(hw))
        }
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        // SAFETY: both references were created by `new` and are released once.
        unsafe {
            ffi::av_buffer_unref(&raw mut self.frames);
            ffi::av_buffer_unref(&raw mut self.device);
        }
    }
}
