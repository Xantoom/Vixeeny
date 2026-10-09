// SPDX-License-Identifier: GPL-3.0-or-later
//! The FFmpeg side of the hardware probe and of `cargo xtask verify-registry`.

use std::ffi::{CStr, CString, c_void};
use std::str::FromStr;

use ffmpeg_next::format::Pixel;
use ffmpeg_next::{Dictionary, Packet, Rational, codec, color, encoder, ffi, frame};

use crate::probe::{Adapter, DriverCaps, Prober};
use crate::registry::{Encoder, Kind, ParamType, PixelFormatSpec, Registry};

/// Asks the GPU drivers what their encoders can do; opens trial sessions with the encoders of the
/// linked FFmpeg for the rest.
pub struct FfmpegProber {
    adapters: Vec<Adapter>,
}

impl FfmpegProber {
    /// `adapters` come from the OS (`vixeeny_platform::gpu_adapters`).
    pub fn new(adapters: Vec<Adapter>) -> Self {
        let _ = ffmpeg_next::init();
        // Trial sessions are noisy and their failures are the point.
        ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Quiet);
        Self { adapters }
    }
}

fn pixel(name: &str) -> Result<Pixel, String> {
    Pixel::from_str(name).map_err(|_| format!("unknown pixel format {name}"))
}

impl Prober for FfmpegProber {
    fn adapters(&self) -> Vec<Adapter> {
        self.adapters.clone()
    }

    fn built_in(&self, encoder: &Encoder) -> bool {
        encoder::find_by_name(&encoder.ffmpeg_encoder).is_some()
    }

    #[cfg(windows)]
    fn driver_caps(&self, adapter: &Adapter) -> Result<Vec<DriverCaps>, String> {
        crate::driver_caps::query(adapter)
    }

    #[cfg(not(windows))]
    fn driver_caps(&self, _adapter: &Adapter) -> Result<Vec<DriverCaps>, String> {
        Err("no capability query on this system".into())
    }

    fn try_open(
        &self,
        encoder: &Encoder,
        adapter: Option<&Adapter>,
        format: &PixelFormatSpec,
        size: (u32, u32),
        hdr: bool,
    ) -> Result<(), String> {
        let codec = encoder::find_by_name(&encoder.ffmpeg_encoder)
            .ok_or_else(|| format!("{} is not built in", encoder.ffmpeg_encoder))?;
        let pix = pixel(&format.ffmpeg)?;
        let mut ctx = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(|e| e.to_string())?;
        ctx.set_width(size.0);
        ctx.set_height(size.1);
        ctx.set_format(pix);
        ctx.set_time_base(Rational(1, 60));
        ctx.set_frame_rate(Some(Rational(60, 1)));
        ctx.set_max_b_frames(0);
        if hdr {
            ctx.set_color_primaries(color::Primaries::BT2020);
            ctx.set_color_transfer_characteristic(color::TransferCharacteristic::SMPTE2084);
            ctx.set_colorspace(color::Space::BT2020NCL);
        }
        let mut options = Dictionary::new();
        if let (Some(adapter), true) = (adapter, encoder.vendor == crate::registry::Vendor::Nvidia)
        {
            options.set("gpu", &adapter.index.to_string());
        }
        let mut opened = ctx.open_with(options).map_err(|e| e.to_string())?;
        let mut frame = frame::Video::new(pix, size.0, size.1);
        frame.set_pts(Some(0));
        opened.send_frame(&frame).map_err(|e| e.to_string())?;
        opened.send_eof().map_err(|e| e.to_string())?;
        let mut packet = Packet::empty();
        opened
            .receive_packet(&mut packet)
            .map_err(|e| format!("no packet: {e}"))
    }
}

/// An option or value of the registry that the real encoder does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub encoder: String,
    pub what: String,
}

/// Looks `name` up in the encoder's private options, then in the generic codec options. When
/// `unit` is given, looks for a constant of that unit instead.
fn find_option(
    codec: &ffmpeg_next::Codec,
    name: &str,
    unit: Option<&CStr>,
) -> Option<*const ffi::AVOption> {
    let name = CString::new(name).ok()?;
    // SAFETY: `priv_class` is either null or a static class; `avcodec_get_class` returns a
    // static class. `av_opt_find` with FAKE_OBJ only reads the class through the pointer to
    // the class pointer we pass, which lives for the call.
    unsafe {
        let mut classes: Vec<*const ffi::AVClass> = Vec::new();
        let private = (*codec.as_ptr()).priv_class;
        if !private.is_null() {
            classes.push(private);
        }
        classes.push(ffi::avcodec_get_class());
        for class in classes {
            let mut holder = class;
            let found = ffi::av_opt_find(
                (&raw mut holder).cast::<c_void>(),
                name.as_ptr(),
                unit.map_or(std::ptr::null(), CStr::as_ptr),
                0,
                ffi::AV_OPT_SEARCH_FAKE_OBJ,
            );
            if !found.is_null() {
                return Some(found);
            }
        }
    }
    None
}

/// Compares the registry with the encoders of the linked FFmpeg (`ffmpeg -h encoder=…`): every
/// option, enum value, preset option and pixel format must exist. Software encoders must be
/// built in; hardware ones must be when `require_hardware` (the Windows build ships them all),
/// otherwise they are skipped when absent.
pub fn verify(registry: &Registry, require_hardware: bool) -> Vec<Mismatch> {
    let _ = ffmpeg_next::init();
    let mut out = Vec::new();
    for e in registry.encoders() {
        let mut bad = |what: String| {
            out.push(Mismatch {
                encoder: e.id.clone(),
                what,
            })
        };
        let Some(codec) = encoder::find_by_name(&e.ffmpeg_encoder) else {
            if e.kind == Kind::Software || require_hardware {
                bad(format!("encoder `{}` is not built in", e.ffmpeg_encoder));
            }
            continue;
        };
        for p in &e.params {
            let Some(option) = find_option(&codec, &p.ffmpeg_option, None) else {
                bad(format!(
                    "param `{}`: no option `{}`",
                    p.key, p.ffmpeg_option
                ));
                continue;
            };
            if p.kind != ParamType::Enum {
                continue;
            }
            // SAFETY: `option` points to a static AVOption.
            let unit = unsafe { (*option).unit };
            if unit.is_null() {
                continue; // a string option: its values cannot be listed
            }
            // SAFETY: `unit` is a static NUL-terminated string.
            let unit = unsafe { CStr::from_ptr(unit) };
            for value in &p.values {
                if p.auto.as_ref().is_some_and(|a| a.to_ffmpeg() == *value) {
                    continue; // never written
                }
                if find_option(&codec, value, Some(unit)).is_none() {
                    bad(format!(
                        "param `{}`: `{}` has no value `{value}`",
                        p.key, p.ffmpeg_option
                    ));
                }
            }
        }
        for name in [
            crate::registry::PresetName::Quality,
            crate::registry::PresetName::Balanced,
            crate::registry::PresetName::Performance,
            crate::registry::PresetName::Small,
        ] {
            for key in e.presets.get(name).keys() {
                if find_option(&codec, key, None).is_none() {
                    bad(format!("preset {name:?}: no option `{key}`"));
                }
            }
        }
        for (mode, options) in &e.rate_control.modes {
            for key in options.keys() {
                if find_option(&codec, key, None).is_none() {
                    bad(format!("rate mode {mode:?}: no option `{key}`"));
                }
            }
        }
        if let Ok(video) = codec.video()
            && let Some(formats) = video.formats()
        {
            let supported: Vec<Pixel> = formats.collect();
            for f in &e.pixel_formats {
                match pixel(&f.ffmpeg) {
                    Ok(pix) if supported.contains(&pix) => {}
                    Ok(_) => bad(format!("pixel format `{}` is not accepted", f.ffmpeg)),
                    Err(m) => bad(m),
                }
            }
        }
    }
    out
}
