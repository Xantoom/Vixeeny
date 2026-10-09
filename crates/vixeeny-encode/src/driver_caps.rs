// SPDX-License-Identifier: GPL-3.0-or-later
//! What a GPU's video encoder can do, as its driver describes it: NVENC (NVIDIA), AMF (AMD) and
//! oneVPL (Intel) each answer a capability query. Nothing is encoded and no encoder is set up, so
//! the probe does not disturb the GPU (a few seconds of trial sessions made the screen flicker on
//! a VRR monitor).
//!
//! The structures below mirror the vendors' C headers (`nvEncodeAPI.h` 13.1, AMF 1.5
//! `public/include`, oneVPL 2.x `mfxcommon.h`); only the parts that are read are spelled out.

use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{GUID, Interface, PCSTR, PCWSTR, s};

use crate::probe::{Adapter, DriverCaps};
use crate::registry::{Chroma, Family, Vendor};

/// The encoders of `adapter`'s GPU, one entry per codec it can encode. `Err` when the driver
/// cannot be asked (no runtime, or one too old to describe itself).
pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, String> {
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

fn load(name: &str) -> Result<HMODULE, String> {
    let name16 = wide(name);
    // SAFETY: a NUL-terminated name; the module stays loaded for the life of the process (the
    // probe is a short-lived child).
    unsafe { LoadLibraryW(PCWSTR(name16.as_ptr())) }.map_err(|e| format!("{name}: {e}"))
}

/// The address of `name` in `module`, as the function type `F`.
///
/// # Safety
/// `F` must be the function pointer type of that export.
unsafe fn symbol<F>(module: HMODULE, name: PCSTR) -> Result<F, String> {
    // SAFETY: the module is loaded and `name` is NUL terminated.
    let found = unsafe { GetProcAddress(module, name) }
        // SAFETY: `name` is a NUL-terminated literal.
        .ok_or_else(|| format!("no {}", unsafe { name.display() }))?;
    // SAFETY: the caller names the right type; both are plain function pointers.
    Ok(unsafe { std::mem::transmute_copy(&found) })
}

/// A D3D11 device on the `index`-th GPU of `vendor_id` (the order of DXGI, like
/// [`Adapter::index`]).
fn device(adapter: &Adapter) -> Result<ID3D11Device, String> {
    // SAFETY: plain DXGI enumeration; every call is checked.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| format!("DXGI factory: {e}"))?;
    let gpu: IDXGIAdapter = (0..)
        // SAFETY: `factory` is live; the index runs until the enumeration reports the end.
        .map_while(|i| unsafe { factory.EnumAdapters1(i) }.ok())
        .filter(|a: &IDXGIAdapter1| {
            // SAFETY: `a` is a live adapter.
            unsafe { a.GetDesc1() }.is_ok_and(|d| d.VendorId == adapter.vendor_id)
        })
        .nth(adapter.index as usize)
        .and_then(|a| a.cast().ok())
        .ok_or_else(|| format!("{} is gone", adapter.name))?;
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
    .map_err(|e| format!("D3D11 device: {e}"))?;
    device.ok_or_else(|| "no D3D11 device".into())
}

fn caps(family: Family, ten_bit: bool, chroma: Vec<Chroma>, max: (u32, u32)) -> DriverCaps {
    DriverCaps {
        family,
        ten_bit,
        chroma,
        max,
    }
}

mod nvenc {
    use super::{Adapter, DriverCaps, GUID, Interface, c_void, caps, device, load, s, symbol};
    use crate::registry::{Chroma, Family};

    /// The API version FFmpeg's NVENC is built with (`nv-codec-headers` of the pinned build). A
    /// driver that cannot open a session at this version is one FFmpeg refuses too.
    const MAJOR: u32 = 13;
    const MINOR: u32 = 1;
    const VERSION: u32 = MAJOR | (MINOR << 24);
    const fn struct_version(v: u32) -> u32 {
        VERSION | (v << 16) | (0x7 << 28)
    }

    // NV_ENC_CAPS
    const WIDTH_MAX: u32 = 16;
    const HEIGHT_MAX: u32 = 17;
    const SUPPORT_YUV444_ENCODE: u32 = 33;
    const SUPPORT_10BIT_ENCODE: u32 = 39;
    const SUPPORT_YUV422_ENCODE: u32 = 59;

    const CODECS: [(Family, GUID); 3] = [
        (
            Family::H264,
            GUID::from_u128(0x6bc82762_4e63_4ca4_aa85_1e50f321f6bf),
        ),
        (
            Family::Hevc,
            GUID::from_u128(0x790cdc88_4522_4d7b_9425_bda9975f7603),
        ),
        (
            Family::Av1,
            GUID::from_u128(0x0a352289_0aa7_4759_862d_5d15cd16d254),
        ),
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

    pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, String> {
        let module = load("nvEncodeAPI64.dll")?;
        // SAFETY: the exports of nvEncodeAPI64.dll, with their documented signatures.
        let (max_version, create): (MaxVersion, CreateInstance) = unsafe {
            (
                symbol(module, s!("NvEncodeAPIGetMaxSupportedVersion"))?,
                symbol(module, s!("NvEncodeAPICreateInstance"))?,
            )
        };
        let mut max = 0;
        // SAFETY: a valid out-pointer.
        if unsafe { max_version(&raw mut max) } != 0 || max < (MAJOR << 4 | MINOR) {
            // FFmpeg would refuse this driver: no NVENC.
            return Ok(Vec::new());
        }
        let mut list = FunctionList {
            version: struct_version(2),
            reserved: 0,
            entries: [std::ptr::null(); 320],
        };
        // SAFETY: `list` is a zeroed function list with its version set.
        if unsafe { create(&raw mut list) } != 0 {
            return Err("NvEncodeAPICreateInstance failed".into());
        }
        let entry = |i: usize| -> Result<*const c_void, String> {
            let f = list.entries[i];
            if f.is_null() {
                Err(format!("NVENC entry {i} missing"))
            } else {
                Ok(f)
            }
        };
        // SAFETY: the entries hold the documented function types at these positions.
        let (open, count, guids, get_caps, destroy): (
            OpenSession,
            GuidCount,
            Guids,
            Caps,
            Destroy,
        ) = unsafe {
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
            let mut param = CapsParam {
                version: struct_version(1),
                caps_to_query: which,
                reserved: [0; 62],
            };
            let mut value = 0;
            // SAFETY: a live session, a valid parameter block and out-pointer.
            let status = unsafe { get_caps(encoder, codec, &raw mut param, &raw mut value) };
            if status == 0 { value } else { 0 }
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
                caps(family, cap(guid, SUPPORT_10BIT_ENCODE) != 0, chroma, max)
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
        assert_eq!(std::mem::offset_of!(OpenParams, api_version), 24);
        assert_eq!(std::mem::size_of::<OpenParams>(), 1552);
        assert_eq!(std::mem::size_of::<CapsParam>(), 256);
        assert_eq!(std::mem::offset_of!(FunctionList, entries), 8);
    }
}

mod amf {
    use super::{Adapter, DriverCaps, Interface, c_void, caps, device, load, s, symbol, wide};
    use crate::registry::{Chroma, Family};

    const COMPONENTS: [(Family, &str); 3] = [
        (Family::H264, "AMFVideoEncoderVCE_AVC"),
        (Family::Hevc, "AMFVideoEncoderHW_HEVC"),
        (Family::Av1, "AMFVideoEncoderHW_AV1"),
    ];
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
    const CAPS_ACCELERATION_TYPE: usize = 13;
    const CAPS_INPUT: usize = 14;
    const IO_WIDTH_RANGE: usize = 3;
    const IO_HEIGHT_RANGE: usize = 4;
    const IO_FORMAT_COUNT: usize = 6;
    const IO_FORMAT_AT: usize = 7;

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
                unsafe {
                    method::<unsafe extern "system" fn(*mut Object) -> i32>(self.0, RELEASE)(self.0)
                };
            }
        }
    }

    type QueryVersion = unsafe extern "C" fn(*mut u64) -> i32;
    type Init = unsafe extern "C" fn(u64, *mut *mut Object) -> i32;

    pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, String> {
        let module = load("amfrt64.dll")?;
        // SAFETY: the exports of amfrt64.dll, with their documented signatures.
        let (version, init): (QueryVersion, Init) = unsafe {
            (
                symbol(module, s!("AMFQueryVersion"))?,
                symbol(module, s!("AMFInit"))?,
            )
        };
        let mut runtime = 0;
        let mut factory = std::ptr::null_mut();
        // SAFETY: valid out-pointers; the runtime's own version is always accepted.
        if unsafe { version(&raw mut runtime) } != 0
            // SAFETY: as above.
            || unsafe { init(runtime, &raw mut factory) } != 0
            || factory.is_null()
        {
            return Err("AMFInit failed".into());
        }
        let device = device(adapter)?;
        let mut context = std::ptr::null_mut();
        // SAFETY: the factory's CreateContext; the factory is static (never released).
        unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut *mut Object) -> i32>(
                factory,
                FACTORY_CREATE_CONTEXT,
            )(factory, &raw mut context)
        };
        if context.is_null() {
            return Err("AMF context failed".into());
        }
        let context = Owned(context);
        // SAFETY: InitDX11 on a live context with a live device.
        let status = unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut c_void, i32) -> i32>(
                context.0,
                CONTEXT_INIT_DX11,
            )(context.0, device.as_raw(), AMF_DX11_0)
        };
        if status != 0 {
            return Ok(Vec::new()); // not an AMD encoder this runtime can drive
        }
        let mut out = Vec::new();
        for (family, id) in COMPONENTS {
            // SAFETY: the calls below follow the AMF headers' vtables on live interfaces.
            if let Some(found) = unsafe { component(factory, &context, family, id) } {
                out.push(found);
            }
        }
        // SAFETY: Terminate on the live context, before it is released.
        unsafe {
            method::<unsafe extern "system" fn(*mut Object) -> i32>(context.0, CONTEXT_TERMINATE)(
                context.0,
            )
        };
        drop(context);
        drop(device);
        Ok(out)
    }

    /// The capabilities of one encoder component, without initialising it.
    ///
    /// # Safety
    /// `factory` and `context` are live AMF interfaces.
    unsafe fn component(
        factory: *mut Object,
        context: &Owned,
        family: Family,
        id: &str,
    ) -> Option<DriverCaps> {
        let id = wide(id);
        let mut component = std::ptr::null_mut();
        // SAFETY: per the caller.
        let status = unsafe {
            method::<
                unsafe extern "system" fn(
                    *mut Object,
                    *mut Object,
                    *const u16,
                    *mut *mut Object,
                ) -> i32,
            >(factory, FACTORY_CREATE_COMPONENT)(
                factory,
                context.0,
                id.as_ptr(),
                &raw mut component,
            )
        };
        if status != 0 || component.is_null() {
            return None;
        }
        let component = Owned(component);
        let mut caps_ptr = std::ptr::null_mut();
        // SAFETY: GetCaps on the live component.
        unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut *mut Object) -> i32>(
                component.0,
                COMPONENT_GET_CAPS,
            )(component.0, &raw mut caps_ptr)
        };
        if caps_ptr.is_null() {
            return None;
        }
        let all = Owned(caps_ptr);
        // SAFETY: GetAccelerationType on the live caps.
        let accel = unsafe {
            method::<unsafe extern "system" fn(*mut Object) -> i32>(all.0, CAPS_ACCELERATION_TYPE)(
                all.0,
            )
        };
        if accel == AMF_ACCEL_NOT_SUPPORTED {
            return None;
        }
        let mut input = std::ptr::null_mut();
        // SAFETY: GetInputCaps on the live caps.
        unsafe {
            method::<unsafe extern "system" fn(*mut Object, *mut *mut Object) -> i32>(
                all.0, CAPS_INPUT,
            )(all.0, &raw mut input)
        };
        if input.is_null() {
            return None;
        }
        let input = Owned(input);
        type Range = unsafe extern "system" fn(*mut Object, *mut i32, *mut i32);
        let (mut w0, mut w1, mut h0, mut h1) = (0, 0, 0, 0);
        // SAFETY: GetWidthRange / GetHeightRange / GetNumOfFormats / GetFormatAt on the live
        // input caps, with valid out-pointers.
        let formats: Vec<i32> = unsafe {
            method::<Range>(input.0, IO_WIDTH_RANGE)(input.0, &raw mut w0, &raw mut w1);
            method::<Range>(input.0, IO_HEIGHT_RANGE)(input.0, &raw mut h0, &raw mut h1);
            let n = method::<unsafe extern "system" fn(*mut Object) -> i32>(
                input.0,
                IO_FORMAT_COUNT,
            )(input.0);
            (0..n)
                .filter_map(|i| {
                    let (mut format, mut native) = (0, false);
                    let status = method::<
                        unsafe extern "system" fn(*mut Object, i32, *mut i32, *mut bool) -> i32,
                    >(input.0, IO_FORMAT_AT)(
                        input.0, i, &raw mut format, &raw mut native
                    );
                    (status == 0).then_some(format)
                })
                .collect()
        };
        let max = (
            u32::try_from(w1).unwrap_or(0),
            u32::try_from(h1).unwrap_or(0),
        );
        Some(caps(
            family,
            formats.contains(&AMF_SURFACE_P010),
            vec![Chroma::C420],
            max,
        ))
    }
}

mod vpl {
    use super::{Adapter, DriverCaps, HMODULE, c_void, caps, load, s, symbol, wide};
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

    // mfxImplDescription (mfxcommon.h, 64-bit): `Impl` at 4, `Dev.DeviceID` at 312 + 16,
    // `Enc` at 504.
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

    /// The oneVPL runtime of the Intel driver: in its DriverStore folder (what the oneVPL
    /// dispatcher looks up), else on the search path.
    fn runtime(adapter: &Adapter) -> Result<HMODULE, String> {
        driver_store(adapter.device_id)
            .and_then(|dir| load(&format!("{dir}\\libmfx64-gen.dll")).ok())
            .map_or_else(|| load("libmfx64-gen.dll"), Ok)
    }

    /// `DriverStorePathForVPL` of the Intel display device `device_id`.
    fn driver_store(device_id: u32) -> Option<String> {
        use windows::Win32::Devices::DeviceAndDriverInstallation::{
            DICS_FLAG_GLOBAL, DIGCF_PRESENT, DIREG_DRV, SP_DEVINFO_DATA,
            SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
            SetupDiGetDeviceInstanceIdW, SetupDiOpenDevRegKey,
        };
        use windows::Win32::System::Registry::{KEY_READ, RegCloseKey, RegQueryValueExW};
        use windows::core::GUID;
        const DISPLAY: GUID = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);
        let wanted = format!("VEN_8086&DEV_{device_id:04X}");
        // SAFETY: SetupAPI enumeration of the present display devices; every handle is closed.
        unsafe {
            let set =
                SetupDiGetClassDevsW(Some(&DISPLAY), PCWSTR::null(), None, DIGCF_PRESENT).ok()?;
            let mut found = None;
            for i in 0.. {
                let mut info = SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                };
                if SetupDiEnumDeviceInfo(set, i, &raw mut info).is_err() {
                    break;
                }
                let mut id = [0u16; 512];
                if SetupDiGetDeviceInstanceIdW(set, &info, Some(&mut id), None).is_err() {
                    continue;
                }
                let id =
                    String::from_utf16_lossy(&id[..id.iter().position(|c| *c == 0).unwrap_or(0)]);
                if !id.to_uppercase().contains(&wanted) {
                    continue;
                }
                let Ok(key) =
                    SetupDiOpenDevRegKey(set, &info, DICS_FLAG_GLOBAL.0, 0, DIREG_DRV, KEY_READ.0)
                else {
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
                    found = Some(
                        String::from_utf16_lossy(&path[..len])
                            .trim_end_matches('\\')
                            .to_owned(),
                    );
                    break;
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
            found
        }
    }
    use windows::core::PCWSTR;

    pub fn query(adapter: &Adapter) -> Result<Vec<DriverCaps>, String> {
        let module = runtime(adapter)?;
        // SAFETY: the exports of the oneVPL runtime, with their documented signatures. A
        // Media SDK runtime (before oneVPL) has neither: the caller falls back.
        let (query, release): (Query, Release) = unsafe {
            (
                symbol(module, s!("MFXQueryImplsDescription"))?,
                symbol(module, s!("MFXReleaseImplDescription"))?,
            )
        };
        let mut n = 0;
        // SAFETY: a valid out-pointer; the array is released below.
        let list = unsafe { query(MFX_IMPLCAPS_IMPLDESCSTRUCTURE, &raw mut n) };
        if list.is_null() {
            return Err("MFXQueryImplsDescription failed".into());
        }
        // SAFETY: `list` holds `n` descriptions, alive until released.
        let descriptions: Vec<*const u8> =
            unsafe { std::slice::from_raw_parts(list, n as usize) }.to_vec();
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
        let text =
            String::from_utf8_lossy(&field[..field.iter().position(|c| *c == 0).unwrap_or(0)]);
        let hex = text.split('/').next().unwrap_or_default();
        u32::from_str_radix(hex.trim_start_matches("0x"), 16).is_ok_and(|id| id == device_id)
    }

    /// # Safety
    /// `enc` is the live encoder description of a runtime.
    unsafe fn encoders(enc: &Encoders) -> Vec<DriverCaps> {
        // SAFETY: arrays of the given lengths, per the oneVPL documentation.
        let slice = |p: *const Codec, n: u16| unsafe {
            if p.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(p, usize::from(n))
            }
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
        out
    }

    #[cfg(test)]
    #[test]
    fn layouts_match_the_header() {
        // Sizes and offsets of mfxcommon.h on 64-bit (computed from the header).
        assert_eq!(std::mem::offset_of!(Encoders, num_codecs), 16);
        assert_eq!(std::mem::offset_of!(Encoders, codecs), 24);
        assert_eq!(std::mem::size_of::<Codec>(), 32);
        assert_eq!(std::mem::offset_of!(Codec, num_profiles), 22);
        assert_eq!(std::mem::offset_of!(Codec, profiles), 24);
        assert_eq!(std::mem::size_of::<Profile>(), 32);
        assert_eq!(std::mem::offset_of!(Profile, num_mem_types), 18);
        assert_eq!(std::mem::offset_of!(Profile, mem_desc), 24);
        assert_eq!(std::mem::size_of::<MemDesc>(), 56);
        assert_eq!(std::mem::offset_of!(MemDesc, height), 16);
        assert_eq!(std::mem::offset_of!(MemDesc, num_color_formats), 42);
        assert_eq!(std::mem::offset_of!(MemDesc, color_formats), 48);
    }
}
