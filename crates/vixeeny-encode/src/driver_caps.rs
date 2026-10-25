// SPDX-License-Identifier: GPL-3.0-or-later
//! What a GPU's video encoder can do, as its driver describes it: NVENC (NVIDIA), AMF (AMD) and oneVPL (Intel) each
//! answer a capability query. Nothing is encoded and no encoder is set up, so the probe does not disturb the GPU (a few
//! seconds of trial sessions made the screen flicker on a VRR monitor).
//!
//! The structures below mirror the vendors' C headers (`nvEncodeAPI.h` 13.1, AMF 1.5 `public/include`, oneVPL 2.x
//! `mfxcommon.h`); only the parts that are read are spelled out.

use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{GUID, Interface, PCSTR, PCWSTR, s};

use crate::probe::{Adapter, DriverCaps, Tools};
use crate::registry::{Chroma, Family, Vendor};

/// Why a driver cannot be asked what it encodes.
#[derive(Debug, thiserror::Error)]
pub enum CapsError {
    /// A system call failed (`what`: the call, or the library loaded).
    #[error("{what}: {source}")]
    Os {
        what: String,
        #[source]
        source: windows::core::Error,
    },
    /// The runtime is there but refuses, or lacks a function.
    #[error("{0}")]
    Runtime(String),
}

fn os(what: impl Into<String>) -> impl FnOnce(windows::core::Error) -> CapsError {
    let what = what.into();
    move |source| CapsError::Os { what, source }
}

/// The encoders of `adapter`'s GPU, one entry per codec it can encode. `Err` when the driver cannot be asked (no
/// runtime, or one too old to describe itself).
pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, CapsError> {
    match adapter.vendor {
        Vendor::Nvidia => nvenc::query(adapter),
        Vendor::Amd => amf::query(adapter),
        Vendor::Intel => vpl::query(adapter),
        Vendor::None => Ok(Vec::new()),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Loads a driver library: a bare name from the system folder only (where the drivers put them), never from the current
/// folder or the `PATH`; a full path from there, with what it needs from its own folder.
fn load(name: &str) -> Result<HMODULE, CapsError> {
    let name16 = wide(name);
    let flags = if name.contains('\\') {
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32
    } else {
        LOAD_LIBRARY_SEARCH_SYSTEM32
    };
    // SAFETY: a NUL-terminated name; the module stays loaded for the life of the process (the probe is a short-lived
    // child).
    unsafe { LoadLibraryExW(PCWSTR(name16.as_ptr()), None, flags) }.map_err(os(name))
}

/// The address of `name` in `module`, as the function type `F`.
///
/// # Safety
/// `F` must be the function pointer type of that export.
unsafe fn symbol<F>(module: HMODULE, name: PCSTR) -> Result<F, CapsError> {
    // SAFETY: the module is loaded and `name` is NUL terminated.
    let found = unsafe { GetProcAddress(module, name) }
        // SAFETY: `name` is a NUL-terminated literal.
        .ok_or_else(|| CapsError::Runtime(format!("no {}", unsafe { name.display() })))?;
    // SAFETY: the caller names the right type; both are plain function pointers.
    Ok(unsafe { std::mem::transmute_copy(&found) })
}

/// A D3D11 device on the `index`-th GPU of `vendor_id` (the order of DXGI, like [`Adapter::index`]).
fn device(adapter: &Adapter) -> Result<ID3D11Device, CapsError> {
    // SAFETY: plain DXGI enumeration; every call is checked.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(os("DXGI factory"))?;
    let gpu: IDXGIAdapter = (0..)
        // SAFETY: `factory` is live; the index runs until the enumeration reports the end.
        .map_while(|i| unsafe { factory.EnumAdapters1(i) }.ok())
        .filter(|a: &IDXGIAdapter1| {
            // SAFETY: `a` is a live adapter.
            unsafe { a.GetDesc1() }.is_ok_and(|d| d.VendorId == adapter.vendor_id)
        })
        .nth(adapter.index as usize)
        .and_then(|a| a.cast().ok())
        .ok_or_else(|| CapsError::Runtime(format!("{} is gone", adapter.name)))?;
    let mut device = None;
    // SAFETY: out-pointer is valid; the adapter outlives the call.
    unsafe {
        D3D11CreateDevice(
            &gpu,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            None,
        )
    }
    .map_err(os("D3D11 device"))?;
    device.ok_or_else(|| CapsError::Runtime("no D3D11 device".into()))
}

fn caps(family: Family, ten_bit: bool, chroma: Vec<Chroma>, max: (u32, u32)) -> DriverCaps {
    DriverCaps { family, ten_bit, chroma, max, tools: None }
}

mod nvenc {
    use super::{Adapter, CapsError, DriverCaps, GUID, Interface, Tools, c_void, caps, device, load, s, symbol};
    use crate::probe::NvencGeneration;
    use crate::registry::{Chroma, Family};

    /// The API version FFmpeg's NVENC is built with (`nv-codec-headers` of the pinned build). A driver that cannot open
    /// a session at this version is one FFmpeg refuses too.
    const MAJOR: u32 = 13;
    const MINOR: u32 = 1;
    const VERSION: u32 = MAJOR | (MINOR << 24);
    const fn struct_version(v: u32) -> u32 {
        VERSION | (v << 16) | (0x7 << 28)
    }

    // NV_ENC_CAPS
    const NUM_MAX_BFRAMES: u32 = 0;
    const WIDTH_MAX: u32 = 16;
    const HEIGHT_MAX: u32 = 17;
    const SUPPORT_YUV444_ENCODE: u32 = 33;
    const SUPPORT_LOOKAHEAD: u32 = 37;
    const SUPPORT_TEMPORAL_AQ: u32 = 38;
    const SUPPORT_10BIT_ENCODE: u32 = 39;
    const SUPPORT_BFRAME_REF_MODE: u32 = 43;
    /// `SUPPORT_BFRAME_REF_MODE` bit: the middle B-frame as a reference.
    const BFRAME_REF_MIDDLE: i32 = 2;
    const NUM_ENCODER_ENGINES: u32 = 49;
    const SUPPORT_LOOKAHEAD_LEVEL: u32 = 56;
    const SUPPORT_YUV422_ENCODE: u32 = 59;

    const CODECS: [(Family, GUID); 3] = [
        (Family::H264, GUID::from_u128(0x6bc82762_4e63_4ca4_aa85_1e50f321f6bf)),
        (Family::Hevc, GUID::from_u128(0x790cdc88_4522_4d7b_9425_bda9975f7603)),
        (Family::Av1, GUID::from_u128(0x0a352289_0aa7_4759_862d_5d15cd16d254)),
    ];

    /// NV_ENCODE_API_FUNCTION_LIST: the entries, after `version` and `reserved`.
    #[repr(C)]
    struct FunctionList {
        version: u32,
        reserved: u32,
        entries: [*const c_void; 320],
    }
    // Positions in `entries`.
    const GET_ENCODE_GUID_COUNT: usize = 1;
    const GET_ENCODE_GUIDS: usize = 4;
    const GET_ENCODE_CAPS: usize = 7;
    const DESTROY_ENCODER: usize = 27;
    const OPEN_ENCODE_SESSION_EX: usize = 29;

    /// NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS
    #[repr(C)]
    struct OpenParams {
        version: u32,
        device_type: u32,
        device: *mut c_void,
        reserved: *mut c_void,
        api_version: u32,
        reserved1: [u32; 253],
        reserved2: [*mut c_void; 64],
    }

    /// NV_ENC_CAPS_PARAM
    #[repr(C)]
    struct CapsParam {
        version: u32,
        caps_to_query: u32,
        reserved: [u32; 62],
    }

    type CreateInstance = unsafe extern "system" fn(*mut FunctionList) -> i32;
    type MaxVersion = unsafe extern "system" fn(*mut u32) -> i32;
    type OpenSession = unsafe extern "system" fn(*mut OpenParams, *mut *mut c_void) -> i32;
    type GuidCount = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
    type Guids = unsafe extern "system" fn(*mut c_void, *mut GUID, u32, *mut u32) -> i32;
    type Caps = unsafe extern "system" fn(*mut c_void, GUID, *mut CapsParam, *mut i32) -> i32;
    type Destroy = unsafe extern "system" fn(*mut c_void) -> i32;

    pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, CapsError> {
        let module = load("nvEncodeAPI64.dll")?;
        // SAFETY: the exports of nvEncodeAPI64.dll, with their documented signatures.
        let (max_version, create): (MaxVersion, CreateInstance) = unsafe {
            (symbol(module, s!("NvEncodeAPIGetMaxSupportedVersion"))?, symbol(module, s!("NvEncodeAPICreateInstance"))?)
        };
        let mut max = 0;
        // SAFETY: a valid out-pointer.
        if unsafe { max_version(&raw mut max) } != 0 || max < (MAJOR << 4 | MINOR) {
            // FFmpeg would refuse this driver: no NVENC.
            return Ok(Vec::new());
        }
        let mut list = FunctionList { version: struct_version(2), reserved: 0, entries: [std::ptr::null(); 320] };
        // SAFETY: `list` is a zeroed function list with its version set.
        if unsafe { create(&raw mut list) } != 0 {
            return Err(CapsError::Runtime("NvEncodeAPICreateInstance failed".into()));
        }
        let entry = |i: usize| -> Result<*const c_void, CapsError> {
            let f = list.entries[i];
            if f.is_null() { Err(CapsError::Runtime(format!("NVENC entry {i} missing"))) } else { Ok(f) }
        };
        // SAFETY: the entries hold the documented function types at these positions.
        let (open, count, guids, get_caps, destroy): (OpenSession, GuidCount, Guids, Caps, Destroy) = unsafe {
            (
                std::mem::transmute::<*const c_void, OpenSession>(entry(OPEN_ENCODE_SESSION_EX)?),
                std::mem::transmute::<*const c_void, GuidCount>(entry(GET_ENCODE_GUID_COUNT)?),
                std::mem::transmute::<*const c_void, Guids>(entry(GET_ENCODE_GUIDS)?),
                std::mem::transmute::<*const c_void, Caps>(entry(GET_ENCODE_CAPS)?),
                std::mem::transmute::<*const c_void, Destroy>(entry(DESTROY_ENCODER)?),
            )
        };
        let device = device(adapter)?;
        let mut params = OpenParams {
            version: struct_version(1),
            device_type: 0, // NV_ENC_DEVICE_TYPE_DIRECTX
            device: device.as_raw(),
            reserved: std::ptr::null_mut(),
            api_version: VERSION,
            reserved1: [0; 253],
            reserved2: [std::ptr::null_mut(); 64],
        };
        let mut encoder = std::ptr::null_mut();
        // SAFETY: valid parameters; the device outlives the session (destroyed below).
        if unsafe { open(&raw mut params, &raw mut encoder) } != 0 || encoder.is_null() {
            // No NVENC on this GPU (or every session is taken).
            return Ok(Vec::new());
        }
        let mut n = 0;
        // SAFETY: a live session and a valid out-pointer.
        unsafe { count(encoder, &raw mut n) };
        let mut found = vec![GUID::zeroed(); n as usize];
        let mut got = 0;
        // SAFETY: `found` holds `n` GUIDs.
        unsafe { guids(encoder, found.as_mut_ptr(), n, &raw mut got) };
        found.truncate(got as usize);
        let cap = |codec: GUID, which: u32| -> i32 {
            let mut param = CapsParam { version: struct_version(1), caps_to_query: which, reserved: [0; 62] };
            let mut value = 0;
            // SAFETY: a live session, a valid parameter block and out-pointer.
            let status = unsafe { get_caps(encoder, codec, &raw mut param, &raw mut value) };
            if status == 0 { value } else { 0 }
        };
        // AV1 came with Ada, 4:2:2 with Blackwell.
        let generation =
            if CODECS.iter().any(|&(_, guid)| found.contains(&guid) && cap(guid, SUPPORT_YUV422_ENCODE) != 0) {
                NvencGeneration::Blackwell
            } else if CODECS.iter().any(|&(family, guid)| family == Family::Av1 && found.contains(&guid)) {
                NvencGeneration::Ada
            } else {
                NvencGeneration::Turing
            };
        let out = CODECS
            .iter()
            .filter(|(_, guid)| found.contains(guid))
            .map(|&(family, guid)| {
                let mut chroma = vec![Chroma::C420];
                if cap(guid, SUPPORT_YUV422_ENCODE) != 0 {
                    chroma.push(Chroma::C422);
                }
                if cap(guid, SUPPORT_YUV444_ENCODE) != 0 {
                    chroma.push(Chroma::C444);
                }
                let max = (
                    u32::try_from(cap(guid, WIDTH_MAX)).unwrap_or(0),
                    u32::try_from(cap(guid, HEIGHT_MAX)).unwrap_or(0),
                );
                let mut found = caps(family, cap(guid, SUPPORT_10BIT_ENCODE) != 0, chroma, max);
                found.tools = Some(Tools {
                    max_bframes: u32::try_from(cap(guid, NUM_MAX_BFRAMES)).unwrap_or(0),
                    bframe_ref_middle: cap(guid, SUPPORT_BFRAME_REF_MODE) & BFRAME_REF_MIDDLE != 0,
                    lookahead: cap(guid, SUPPORT_LOOKAHEAD) != 0,
                    temporal_aq: cap(guid, SUPPORT_TEMPORAL_AQ) != 0,
                    engine_fps: 0,
                    amf_throughput: [0; 4],
                    engines: u32::try_from(cap(guid, NUM_ENCODER_ENGINES)).unwrap_or(0),
                    lookahead_level: u32::try_from(cap(guid, SUPPORT_LOOKAHEAD_LEVEL)).unwrap_or(0),
                    generation,
                });
                found
            })
            .collect();
        // SAFETY: the session opened above, destroyed once.
        unsafe { destroy(encoder) };
        drop(device);
        Ok(out)
    }

    #[cfg(test)]
    #[test]
    fn layouts_match_the_header() {
        // Offsets of nvEncodeAPI.h on 64-bit Windows.
        assert_eq!(
            std::mem::offset_of!(OpenParams, api_version),
            24,
            "nvEncodeAPI.h: OpenParams.api_version at byte 24"
        );
        assert_eq!(std::mem::size_of::<OpenParams>(), 1552, "nvEncodeAPI.h: OpenParams is 1552 bytes");
        assert_eq!(std::mem::size_of::<CapsParam>(), 256, "nvEncodeAPI.h: CapsParam is 256 bytes");
        assert_eq!(std::mem::offset_of!(FunctionList, entries), 8, "nvEncodeAPI.h: FunctionList.entries at byte 8");
    }
}

mod amf {
    use super::{Adapter, CapsError, DriverCaps, Interface, c_void, caps, device, load, s, symbol, wide};
    use crate::probe::{NvencGeneration, Tools};
    use crate::registry::{Chroma, Family};

    /// The names one codec's component goes by (VideoEncoderVCE.h, VideoEncoderHEVC.h, VideoEncoderAV1.h).
    struct Codec {
        family: Family,
        component: &'static str,
        usage: &'static str,
        preset: &'static str,
        /// The speed, balanced and quality presets.
        presets: [i64; 3],
        /// The capability saying it has B-frames (none for HEVC in FFmpeg), then the B-frame pattern and the most
        /// consecutive ones.
        bframes: Option<(&'static str, &'static str, &'static str)>,
        preanalysis: &'static str,
        max_throughput: &'static str,
    }

    const COMPONENTS: [Codec; 3] = [
        Codec {
            family: Family::H264,
            component: "AMFVideoEncoderVCE_AVC",
            usage: "Usage",
            preset: "QualityPreset",
            presets: [1, 0, 2],
            bframes: Some(("BFrames", "BPicturesPattern", "MaxConsecutiveBPictures")),
            preanalysis: "PreAnalysis",
            max_throughput: "MaxThroughput",
        },
        Codec {
            family: Family::Hevc,
            component: "AMFVideoEncoderHW_HEVC",
            usage: "HevcUsage",
            preset: "HevcQualityPreset",
            presets: [10, 5, 0],
            bframes: None,
            preanalysis: "HevcPreAnalysis",
            max_throughput: "HevcMaxThroughput",
        },
        Codec {
            family: Family::Av1,
            component: "AMFVideoEncoderHW_AV1",
            usage: "Av1Usage",
            preset: "Av1QualityPreset",
            presets: [100, 70, 30],
            bframes: Some(("AV1BFrames", "Av1BPicturesPattern", "Av1MaxConsecutiveBPictures")),
            preanalysis: "Av1PreAnalysis",
            max_throughput: "Av1CapMaxThroughput",
        },
    ];
    /// The transcoding usage, Vixeeny's, the same value for every codec.
    const USAGE_TRANSCODING: i64 = 0;
    /// The B-frames whose cost is asked (see [`crate::registry::Encoder::fit_tools`]).
    const ASKED_BFRAMES: i64 = 2;
    /// The most B-frames FFmpeg gives AMF.
    const MAX_BFRAMES: u32 = 3;
    /// Frames of 1080p a second at the balanced preset, the slowest chip of each generation in rigaya's measurements
    /// (Dec. 2025, driver 25.11.1): Radeon 860M for the engines with AV1 (RDNA 3 and later, 194 for H.264), Ryzen 3
    /// 3200G's Vega for the older ones (107).
    const ENGINE_FPS_AV1: u32 = 194;
    const ENGINE_FPS_OLDER: u32 = 107;
    const AMF_DX11_0: i32 = 110;
    const AMF_SURFACE_P010: i32 = 10;
    const AMF_ACCEL_NOT_SUPPORTED: i32 = -1;

    // Positions in the C vtables of the AMF headers.
    const RELEASE: usize = 1;
    const FACTORY_CREATE_CONTEXT: usize = 0;
    const FACTORY_CREATE_COMPONENT: usize = 1;
    const CONTEXT_TERMINATE: usize = 13;
    const CONTEXT_INIT_DX11: usize = 18;
    const COMPONENT_GET_CAPS: usize = 26;
    const STORAGE_SET_PROPERTY: usize = 3;
    const STORAGE_GET_PROPERTY: usize = 4;
    const CAPS_ACCELERATION_TYPE: usize = 13;
    const CAPS_INPUT: usize = 14;
    const IO_WIDTH_RANGE: usize = 3;
    const IO_HEIGHT_RANGE: usize = 4;
    const IO_FORMAT_COUNT: usize = 6;
    const IO_FORMAT_AT: usize = 7;

    const AMF_VARIANT_BOOL: i32 = 1;
    const AMF_VARIANT_INT64: i32 = 2;

    /// `AMFVariantStruct`: its type, then a union of 16 bytes aligned on 8.
    #[repr(C)]
    struct Variant {
        kind: i32,
        value: [u64; 2],
    }
    const _: () = assert!(std::mem::size_of::<Variant>() == 24, "AMFVariantStruct is 24 bytes in the AMF headers");

    /// An AMF interface: a pointer to its vtable.
    #[repr(C)]
    struct Object {
        vtable: *const *const c_void,
    }

    /// Entry `i` of `object`'s vtable as the function type `F`.
    ///
    /// # Safety
    /// `object` is a live AMF interface and `F` the type of that entry.
    unsafe fn method<F>(object: *mut Object, i: usize) -> F {
        // SAFETY: per the caller; vtables are arrays of function pointers.
        unsafe { std::mem::transmute_copy(&*(*object).vtable.add(i)) }
    }

    /// Releases an AMF interface when dropped.
    struct Owned(*mut Object);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a live interface; `Release` is entry 1 of every vtable.
                unsafe { method::<unsafe extern "system" fn(*mut Object) -> i32>(self.0, RELEASE)(self.0) };
            }
        }
    }

    type QueryVersion = unsafe extern "C" fn(*mut u64) -> i32;
    type Init = unsafe extern "C" fn(u64, *mut *mut Object) -> i32;

    pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, CapsError> {
        let module = load("amfrt64.dll")?;
        // SAFETY: the exports of amfrt64.dll, with their documented signatures.
        let (version, init): (QueryVersion, Init) =
            unsafe { (symbol(module, s!("AMFQueryVersion"))?, symbol(module, s!("AMFInit"))?) };
        let mut runtime = 0;
        let mut factory = std::ptr::null_mut();
        // SAFETY: valid out-pointers; the runtime's own version is always accepted.
        if unsafe { version(&raw mut runtime) } != 0
            // SAFETY: as above.
            || unsafe { init(runtime, &raw mut factory) } != 0
            || factory.is_null()
        {
            return Err(CapsError::Runtime("AMFInit failed".into()));
        }
        let device = device(adapter)?;
        let mut context = std::ptr::null_mut();
        // SAFETY: the factory's CreateContext; the factory is static (never released).
        unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut *mut Object) -> i32>(factory, FACTORY_CREATE_CONTEXT)(
                factory,
                &raw mut context,
            )
        };
        if context.is_null() {
            return Err(CapsError::Runtime("AMF context failed".into()));
        }
        let context = Owned(context);
        // SAFETY: InitDX11 on a live context with a live device.
        let status = unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut c_void, i32) -> i32>(context.0, CONTEXT_INIT_DX11)(
                context.0,
                device.as_raw(),
                AMF_DX11_0,
            )
        };
        if status != 0 {
            return Ok(Vec::new()); // not an AMD encoder this runtime can drive
        }
        let mut out = Vec::new();
        for codec in &COMPONENTS {
            // SAFETY: the calls below follow the AMF headers' vtables on live interfaces.
            let found = unsafe { component(factory, &context, codec) };
            out.extend(found);
        }
        let engine_fps = if out.iter().any(|c| c.family == Family::Av1) { ENGINE_FPS_AV1 } else { ENGINE_FPS_OLDER };
        for found in &mut out {
            if let Some(tools) = &mut found.tools {
                tools.engine_fps = engine_fps;
            }
        }
        // SAFETY: Terminate on the live context, before it is released.
        unsafe { method::<unsafe extern "system" fn(*mut Object) -> i32>(context.0, CONTEXT_TERMINATE)(context.0) };
        drop(context);
        drop(device);
        Ok(out)
    }

    /// The capabilities of one encoder component, without initialising it.
    ///
    /// # Safety
    /// `factory` and `context` are live AMF interfaces.
    unsafe fn component(factory: *mut Object, context: &Owned, codec: &Codec) -> Option<DriverCaps> {
        let id = wide(codec.component);
        let mut component = std::ptr::null_mut();
        // SAFETY: per the caller.
        let status = unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut Object, *const u16, *mut *mut Object) -> i32>(
                factory,
                FACTORY_CREATE_COMPONENT,
            )(factory, context.0, id.as_ptr(), &raw mut component)
        };
        if status != 0 || component.is_null() {
            return None;
        }
        let component = Owned(component);
        // SAFETY: GetCaps on the live component.
        let all = unsafe { caps_of(&component) }?;
        // SAFETY: GetAccelerationType on the live caps.
        let accel =
            unsafe { method::<unsafe extern "system" fn(*mut Object) -> i32>(all.0, CAPS_ACCELERATION_TYPE)(all.0) };
        if accel == AMF_ACCEL_NOT_SUPPORTED {
            return None;
        }
        // SAFETY: GetProperty on the live caps.
        let flag = |name: &str| unsafe { flag(all.0, name) };
        let bframes = codec.bframes.filter(|(cap, _, _)| flag(cap));
        let tools = Tools {
            max_bframes: if bframes.is_some() { MAX_BFRAMES } else { 0 },
            bframe_ref_middle: false,
            lookahead: flag(codec.preanalysis),
            temporal_aq: false,
            engine_fps: 0,
            // SAFETY: the live component.
            amf_throughput: unsafe { throughput(&component, codec, bframes.is_some()) },
            engines: 0,
            lookahead_level: 0,
            generation: NvencGeneration::Turing,
        };
        let mut input = std::ptr::null_mut();
        // SAFETY: GetInputCaps on the live caps.
        unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut *mut Object) -> i32>(all.0, CAPS_INPUT)(
                all.0,
                &raw mut input,
            )
        };
        if input.is_null() {
            return None;
        }
        let input = Owned(input);
        type Range = unsafe extern "system" fn(*mut Object, *mut i32, *mut i32);
        let (mut w0, mut w1, mut h0, mut h1) = (0, 0, 0, 0);
        // SAFETY: GetWidthRange / GetHeightRange / GetNumOfFormats / GetFormatAt on the live input caps, with valid
        // out-pointers.
        let formats: Vec<i32> = unsafe {
            method::<Range>(input.0, IO_WIDTH_RANGE)(input.0, &raw mut w0, &raw mut w1);
            method::<Range>(input.0, IO_HEIGHT_RANGE)(input.0, &raw mut h0, &raw mut h1);
            let n = method::<unsafe extern "system" fn(*mut Object) -> i32>(input.0, IO_FORMAT_COUNT)(input.0);
            (0..n)
                .filter_map(|i| {
                    let (mut format, mut native) = (0, false);
                    let status = method::<unsafe extern "system" fn(*mut Object, i32, *mut i32, *mut bool) -> i32>(
                        input.0,
                        IO_FORMAT_AT,
                    )(input.0, i, &raw mut format, &raw mut native);
                    (status == 0).then_some(format)
                })
                .collect()
        };
        let max = (u32::try_from(w1).unwrap_or(0), u32::try_from(h1).unwrap_or(0));
        let mut found = caps(codec.family, formats.contains(&AMF_SURFACE_P010), vec![Chroma::C420], max);
        found.tools = Some(tools);
        Some(found)
    }

    /// The capabilities of `component` as its properties stand.
    ///
    /// # Safety
    /// `component` is a live `AMFComponent`.
    unsafe fn caps_of(component: &Owned) -> Option<Owned> {
        let mut caps = std::ptr::null_mut();
        // SAFETY: GetCaps on the live component, with a valid out-pointer.
        unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut *mut Object) -> i32>(component.0, COMPONENT_GET_CAPS)(
                component.0,
                &raw mut caps,
            )
        };
        (!caps.is_null()).then_some(Owned(caps))
    }

    /// The macroblocks a second the engine keeps up with at the speed, balanced and quality presets, then the quality
    /// one with [`ASKED_BFRAMES`] (0: not said). The driver answers for the properties set (OBS lowers its preset on
    /// the same answer), without the encoder being opened.
    ///
    /// # Safety
    /// `component` is a live `AMFComponent`, not initialised.
    unsafe fn throughput(component: &Owned, codec: &Codec, bframes: bool) -> [u64; 4] {
        let set = |name: &str, value: i64| {
            let name = wide(name);
            let value = Variant { kind: AMF_VARIANT_INT64, value: [value as u64, 0] };
            // SAFETY: SetProperty of AMFPropertyStorage on the live component; the variant is passed by value, as the C
            // header declares it.
            unsafe {
                method::<unsafe extern "system" fn(*mut Object, *const u16, Variant) -> i32>(
                    component.0,
                    STORAGE_SET_PROPERTY,
                )(component.0, name.as_ptr(), value)
            }
        };
        let read = || {
            // SAFETY: per this function's caller.
            unsafe { caps_of(component) }
                // SAFETY: the live caps just taken.
                .and_then(|caps| unsafe { int(caps.0, codec.max_throughput) })
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0)
        };
        let mut out = [0; 4];
        if set(codec.usage, USAGE_TRANSCODING) != 0 {
            return out;
        }
        for (slot, preset) in out.iter_mut().zip(codec.presets) {
            if set(codec.preset, preset) == 0 {
                *slot = read();
            }
        }
        if let Some((_, pattern, consecutive)) = codec.bframes.filter(|_| bframes)
            && set(codec.preset, codec.presets[2]) == 0
            && set(consecutive, ASKED_BFRAMES) == 0
            && set(pattern, ASKED_BFRAMES) == 0
        {
            out[3] = read();
        }
        out
    }

    /// The integer capability `name` of `caps`.
    ///
    /// # Safety
    /// `caps` is a live `AMFCaps`.
    unsafe fn int(caps: *mut Object, name: &str) -> Option<i64> {
        let name = wide(name);
        let mut value = Variant { kind: 0, value: [0; 2] };
        // SAFETY: GetProperty of AMFPropertyStorage, with a valid out-pointer; an integer holds no reference to
        // release.
        let status = unsafe {
            method::<unsafe extern "system" fn(*mut Object, *const u16, *mut Variant) -> i32>(
                caps,
                STORAGE_GET_PROPERTY,
            )(caps, name.as_ptr(), &raw mut value)
        };
        (status == 0 && value.kind == AMF_VARIANT_INT64).then_some(value.value[0] as i64)
    }

    /// The boolean capability `name` of `caps` (false when it lacks it).
    ///
    /// # Safety
    /// `caps` is a live `AMFCaps`.
    unsafe fn flag(caps: *mut Object, name: &str) -> bool {
        let name = wide(name);
        let mut value = Variant { kind: 0, value: [0; 2] };
        // SAFETY: GetProperty of AMFPropertyStorage, with a valid out-pointer; a boolean holds no reference to release.
        let status = unsafe {
            method::<unsafe extern "system" fn(*mut Object, *const u16, *mut Variant) -> i32>(
                caps,
                STORAGE_GET_PROPERTY,
            )(caps, name.as_ptr(), &raw mut value)
        };
        status == 0 && value.kind == AMF_VARIANT_BOOL && value.value[0] & 0xff != 0
    }
}

mod vpl {
    use super::{Adapter, CapsError, DriverCaps, HMODULE, c_void, caps, load, s, symbol, wide};
    use crate::probe::{NOT_DESCRIBED, NvencGeneration, Tools};
    use crate::registry::{Chroma, Family};

    const fn fourcc(code: &[u8; 4]) -> u32 {
        u32::from_le_bytes(*code)
    }
    const CODECS: [(Family, u32); 4] = [
        (Family::H264, fourcc(b"AVC ")),
        (Family::Hevc, fourcc(b"HEVC")),
        (Family::Av1, fourcc(b"AV1 ")),
        (Family::Vp9, fourcc(b"VP9 ")),
    ];
    const P010: u32 = fourcc(b"P010");
    const Y210: u32 = fourcc(b"Y210");
    const YUY2: u32 = fourcc(b"YUY2");
    const AYUV: u32 = fourcc(b"AYUV");
    const Y410: u32 = fourcc(b"Y410");
    const MFX_IMPLCAPS_IMPLDESCSTRUCTURE: i32 = 1;
    const MFX_IMPL_TYPE_HARDWARE: u32 = 2;

    // mfxImplDescription (mfxcommon.h, 64-bit): `Impl` at 4, `Dev.DeviceID` at 312 + 16, `Enc` at 504.
    const IMPL: usize = 4;
    const DEVICE_ID: usize = 328;
    const ENC: usize = 504;

    /// mfxEncoderDescription, from `NumCodecs`.
    #[repr(C)]
    struct Encoders {
        version: u16,
        reserved: [u16; 7],
        num_codecs: u16,
        codecs: *const Codec,
    }
    #[repr(C)]
    struct Codec {
        codec_id: u32,
        max_level: u16,
        bidirectional: u16,
        reserved: [u16; 7],
        num_profiles: u16,
        profiles: *const Profile,
    }
    #[repr(C)]
    struct Profile {
        profile: u32,
        reserved: [u16; 7],
        num_mem_types: u16,
        mem_desc: *const MemDesc,
    }
    #[repr(C)]
    struct Range {
        min: u32,
        max: u32,
        step: u32,
    }
    #[repr(C)]
    struct MemDesc {
        handle_type: i32,
        width: Range,
        height: Range,
        reserved: [u16; 7],
        num_color_formats: u16,
        color_formats: *const u32,
    }

    type Query = unsafe extern "C" fn(i32, *mut u32) -> *mut *const u8;
    type Release = unsafe extern "C" fn(*mut c_void) -> i32;

    /// The oneVPL runtime of the Intel driver: in its DriverStore folder (what the oneVPL dispatcher looks up), else on
    /// the search path.
    fn runtime(adapter: &Adapter) -> Result<HMODULE, CapsError> {
        driver_store(adapter.device_id)
            .and_then(|dir| load(&format!("{dir}\\libmfx64-gen.dll")).ok())
            .map_or_else(|| load("libmfx64-gen.dll"), Ok)
    }

    /// `DriverStorePathForVPL` of the Intel display device `device_id`.
    fn driver_store(device_id: u32) -> Option<String> {
        use windows::Win32::Devices::DeviceAndDriverInstallation::{
            DICS_FLAG_GLOBAL, DIGCF_PRESENT, DIREG_DRV, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList,
            SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiOpenDevRegKey,
        };
        use windows::Win32::System::Registry::{KEY_READ, RegCloseKey, RegQueryValueExW};
        use windows::core::GUID;
        const DISPLAY: GUID = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);
        let wanted = format!("VEN_8086&DEV_{device_id:04X}");
        // SAFETY: SetupAPI enumeration of the present display devices; every handle is closed.
        unsafe {
            let set = SetupDiGetClassDevsW(Some(&DISPLAY), PCWSTR::null(), None, DIGCF_PRESENT).ok()?;
            let mut found = None;
            for i in 0.. {
                let mut info =
                    SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
                if SetupDiEnumDeviceInfo(set, i, &raw mut info).is_err() {
                    break;
                }
                let mut id = [0u16; 512];
                if SetupDiGetDeviceInstanceIdW(set, &info, Some(&mut id), None).is_err() {
                    continue;
                }
                let id = String::from_utf16_lossy(&id[..id.iter().position(|c| *c == 0).unwrap_or(0)]);
                if !id.to_uppercase().contains(&wanted) {
                    continue;
                }
                let Ok(key) = SetupDiOpenDevRegKey(set, &info, DICS_FLAG_GLOBAL.0, 0, DIREG_DRV, KEY_READ.0) else {
                    continue;
                };
                let name = wide("DriverStorePathForVPL");
                let mut path = [0u16; 1024];
                let mut size = std::mem::size_of_val(&path) as u32;
                let read = RegQueryValueExW(
                    key,
                    PCWSTR(name.as_ptr()),
                    None,
                    None,
                    Some(path.as_mut_ptr().cast()),
                    Some(&raw mut size),
                );
                let _ = RegCloseKey(key);
                if read.is_ok() {
                    let len = path.iter().position(|c| *c == 0).unwrap_or(0);
                    found = Some(String::from_utf16_lossy(&path[..len]).trim_end_matches('\\').to_owned());
                    break;
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
            found
        }
    }
    use windows::core::PCWSTR;

    pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, CapsError> {
        let module = runtime(adapter)?;
        // SAFETY: the exports of the oneVPL runtime, with their documented signatures. A Media SDK runtime (before
        // oneVPL) has neither: the caller falls back.
        let (query, release): (Query, Release) = unsafe {
            (symbol(module, s!("MFXQueryImplsDescription"))?, symbol(module, s!("MFXReleaseImplDescription"))?)
        };
        let mut n = 0;
        // SAFETY: a valid out-pointer; the array is released below.
        let list = unsafe { query(MFX_IMPLCAPS_IMPLDESCSTRUCTURE, &raw mut n) };
        if list.is_null() {
            return Err(CapsError::Runtime("MFXQueryImplsDescription failed".into()));
        }
        // SAFETY: `list` holds `n` descriptions, alive until released.
        let descriptions: Vec<*const u8> = unsafe { std::slice::from_raw_parts(list, n as usize) }.to_vec();
        // SAFETY: each description is an mfxImplDescription (offsets above).
        let ours = unsafe {
            let hardware: Vec<*const u8> = descriptions
                .into_iter()
                .filter(|d| (*d).add(IMPL).cast::<u32>().read_unaligned() == MFX_IMPL_TYPE_HARDWARE)
                .collect();
            hardware
                .iter()
                .copied()
                .find(|d| device_matches(*d, adapter.device_id))
                .or_else(|| hardware.first().copied())
        };
        let out = ours.map_or_else(Vec::new, |d| {
            // SAFETY: `d` is a live mfxImplDescription; `Enc` is at `ENC`.
            unsafe { encoders(&*d.add(ENC).cast::<Encoders>()) }
        });
        // SAFETY: the array returned by the query, released once.
        unsafe { release(list.cast()) };
        Ok(out)
    }

    /// `Dev.DeviceID` reads "<hex device id>/<n>".
    ///
    /// # Safety
    /// `description` is a live mfxImplDescription.
    unsafe fn device_matches(description: *const u8, device_id: u32) -> bool {
        // SAFETY: a NUL-terminated field of 128 bytes.
        let field = unsafe { std::slice::from_raw_parts(description.add(DEVICE_ID), 128) };
        let text = String::from_utf8_lossy(&field[..field.iter().position(|c| *c == 0).unwrap_or(0)]);
        let hex = text.split('/').next().unwrap_or_default();
        u32::from_str_radix(hex.trim_start_matches("0x"), 16).is_ok_and(|id| id == device_id)
    }

    /// # Safety
    /// `enc` is the live encoder description of a runtime.
    unsafe fn encoders(enc: &Encoders) -> Vec<DriverCaps> {
        // SAFETY: arrays of the given lengths, per the oneVPL documentation.
        let slice = |p: *const Codec, n: u16| unsafe {
            if p.is_null() { &[][..] } else { std::slice::from_raw_parts(p, usize::from(n)) }
        };
        let mut out = Vec::new();
        for codec in slice(enc.codecs, enc.num_codecs) {
            let Some(&(family, _)) = CODECS.iter().find(|(_, id)| *id == codec.codec_id) else {
                continue;
            };
            let mut formats = Vec::new();
            let mut max = (0, 0);
            // SAFETY: as above.
            unsafe {
                let profiles = if codec.profiles.is_null() {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(codec.profiles, usize::from(codec.num_profiles))
                };
                for p in profiles {
                    let mems = if p.mem_desc.is_null() {
                        &[][..]
                    } else {
                        std::slice::from_raw_parts(p.mem_desc, usize::from(p.num_mem_types))
                    };
                    for m in mems {
                        max = (max.0.max(m.width.max), max.1.max(m.height.max));
                        if !m.color_formats.is_null() {
                            formats.extend_from_slice(std::slice::from_raw_parts(
                                m.color_formats,
                                usize::from(m.num_color_formats),
                            ));
                        }
                    }
                }
            }
            let mut chroma = vec![Chroma::C420];
            if formats.iter().any(|f| [YUY2, Y210].contains(f)) {
                chroma.push(Chroma::C422);
            }
            if formats.iter().any(|f| [AYUV, Y410].contains(f)) {
                chroma.push(Chroma::C444);
            }
            out.push(caps(family, formats.contains(&P010), chroma, max));
        }
        // oneVPL describes no coding tool: only the speed of the media engine, known from its generation.
        let av1 = out.iter().any(|c| c.family == Family::Av1);
        for found in &mut out {
            found.tools = Some(Tools {
                max_bframes: NOT_DESCRIBED,
                bframe_ref_middle: false,
                lookahead: false,
                temporal_aq: false,
                engine_fps: engine_fps(av1, found.family),
                amf_throughput: [0; 4],
                engines: 0,
                lookahead_level: 0,
                generation: NvencGeneration::Turing,
            });
        }
        out
    }

    /// Frames of 1080p a second at the medium preset, the slowest chip of each generation in rigaya's measurements
    /// (Dec. 2025, driver 32.0.101.8250): Core Ultra 7 258V for the engines with AV1 (Arc, Meteor Lake and later), else
    /// Core i5-2410M for H.264 and Core i5-7500 for HEVC (its weakest codec there, VP9 taken as slow).
    fn engine_fps(av1: bool, family: Family) -> u32 {
        match (av1, family) {
            (true, _) => 277,
            (false, Family::H264) => 141,
            (false, _) => 80,
        }
    }

    #[cfg(test)]
    #[test]
    fn layouts_match_the_header() {
        // Sizes and offsets of mfxcommon.h on 64-bit (computed from the header).
        assert_eq!(std::mem::offset_of!(Encoders, num_codecs), 16, "mfxcommon.h: Encoders.num_codecs at byte 16");
        assert_eq!(std::mem::offset_of!(Encoders, codecs), 24, "mfxcommon.h: Encoders.codecs at byte 24");
        assert_eq!(std::mem::size_of::<Codec>(), 32, "mfxcommon.h: Codec is 32 bytes");
        assert_eq!(std::mem::offset_of!(Codec, num_profiles), 22, "mfxcommon.h: Codec.num_profiles at byte 22");
        assert_eq!(std::mem::offset_of!(Codec, profiles), 24, "mfxcommon.h: Codec.profiles at byte 24");
        assert_eq!(std::mem::size_of::<Profile>(), 32, "mfxcommon.h: Profile is 32 bytes");
        assert_eq!(std::mem::offset_of!(Profile, num_mem_types), 18, "mfxcommon.h: Profile.num_mem_types at byte 18");
        assert_eq!(std::mem::offset_of!(Profile, mem_desc), 24, "mfxcommon.h: Profile.mem_desc at byte 24");
        assert_eq!(std::mem::size_of::<MemDesc>(), 56, "mfxcommon.h: MemDesc is 56 bytes");
        assert_eq!(std::mem::offset_of!(MemDesc, height), 16, "mfxcommon.h: MemDesc.height at byte 16");
        assert_eq!(
            std::mem::offset_of!(MemDesc, num_color_formats),
            42,
            "mfxcommon.h: MemDesc.num_color_formats at byte 42"
        );
        assert_eq!(std::mem::offset_of!(MemDesc, color_formats), 48, "mfxcommon.h: MemDesc.color_formats at byte 48");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn driver_libraries_load_from_the_system_folder() {
        assert!(super::load("kernel32.dll").is_ok());
        // A library of the system folder by its full path, with what it needs beside it.
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(super::load(&format!(r"{system}\System32\kernel32.dll")).is_ok());
        // An NVIDIA driver is found where it installs its library.
        if std::path::Path::new(&format!(r"{system}\System32\nvEncodeAPI64.dll")).exists() {
            assert!(super::load("nvEncodeAPI64.dll").is_ok());
        }
        assert!(super::load("not-a-driver-vixeeny.dll").is_err());
    }
}
