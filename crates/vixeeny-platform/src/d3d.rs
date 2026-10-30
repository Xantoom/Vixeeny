// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct3D 11 helpers shared by the shaders of the capture and of the encoder.

use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_UNKNOWN, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1};
use windows::core::{Interface, PCSTR, s};

/// A Direct3D step that failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum D3dError {
    /// `call` returned an error.
    #[error("{call}: {source}")]
    Call {
        call: &'static str,
        #[source]
        source: windows::core::Error,
    },
    /// The call succeeded without what it makes (no object), or what it was given was wrong.
    #[error("{0}")]
    Missing(&'static str),
    /// A shader that does not compile, with the compiler's messages.
    #[error("D3DCompile: {0}")]
    Compile(String),
}

impl From<D3dError> for windows::core::Error {
    fn from(e: D3dError) -> Self {
        match e {
            D3dError::Call { source, .. } => source,
            other => Self::new(E_FAIL, other.to_string()),
        }
    }
}

/// Turns the error of `call` into a [`D3dError`], for `map_err`.
pub fn call(call: &'static str) -> impl Fn(windows::core::Error) -> D3dError + Copy {
    move |source| D3dError::Call { call, source }
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob exposes `GetBufferSize` bytes at `GetBufferPointer` while alive.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

/// The bytecode of the function `entry` of the HLSL `source`, for `target` (`cs_5_0`…).
pub fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>, D3dError> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source is a live string of the given length; out-pointers are valid.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("vixeeny"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &raw mut code,
            Some(&raw mut errors),
        )
    };
    match (compiled, code) {
        (Ok(()), Some(code)) => Ok(blob_bytes(&code).to_vec()),
        (result, _) => Err(D3dError::Compile(format!(
            "{} {}",
            result.err().map(|e| e.to_string()).unwrap_or_default(),
            errors.map(|e| String::from_utf8_lossy(blob_bytes(&e)).into_owned()).unwrap_or_default()
        ))),
    }
}

/// The `index`-th graphics card of the vendor `vendor_id` (its PCI id).
pub fn adapter(vendor_id: u32, index: usize) -> Option<IDXGIAdapter> {
    // SAFETY: plain DXGI enumeration; every call is checked.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    (0..)
        // SAFETY: `factory` is live; the index runs until the enumeration reports the end.
        .map_while(|i| unsafe { factory.EnumAdapters1(i) }.ok())
        // SAFETY: a live adapter.
        .filter(|a: &IDXGIAdapter1| unsafe { a.GetDesc1() }.is_ok_and(|d| d.VendorId == vendor_id))
        .nth(index)
        .and_then(|a| a.cast().ok())
}

/// A Direct3D 11 device made with `flags`, and its immediate context: of `driver`, or on `adapter` when given.
pub fn device(
    adapter: Option<&IDXGIAdapter>,
    driver: D3D_DRIVER_TYPE,
    flags: D3D11_CREATE_DEVICE_FLAG,
) -> Result<(ID3D11Device, ID3D11DeviceContext), D3dError> {
    let (mut device, mut context) = (None, None);
    // An explicit adapter needs the "unknown" driver type.
    let driver = if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { driver };
    // SAFETY: out-pointers are valid; the adapter (when given) outlives the call; no feature-level list or software
    // module.
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
            HMODULE::default(),
            flags,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .map_err(call("D3D11CreateDevice"))?;
    Ok((device.ok_or(D3dError::Missing("no D3D11 device"))?, context.ok_or(D3dError::Missing("no D3D11 context"))?))
}

/// Calls `read` with the rows of `staging` (`height` of them, the stride apart) once the graphics card has written
/// them, and gives back what it returns.
pub fn read_staging<T>(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    height: u32,
    read: impl FnOnce(&[u8], usize) -> T,
) -> Result<T, D3dError> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: a staging texture with CPU read access; `mapped` is a valid out-pointer.
    unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }.map_err(call("Map"))?;
    let stride = mapped.RowPitch as usize;
    // SAFETY: a mapped texture exposes `RowPitch × Height` readable bytes until `Unmap`.
    let rows = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * height as usize) };
    let out = read(rows, stride);
    // SAFETY: matches the successful `Map` above.
    unsafe { context.Unmap(staging, 0) };
    Ok(out)
}

/// A staging texture the processor reads a copy of `texture` from: its format, and its size or `size`.
pub fn staging_for(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
    size: Option<(u32, u32)>,
) -> Result<ID3D11Texture2D, D3dError> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { texture.GetDesc(&raw mut desc) };
    self::texture(device, size.unwrap_or((desc.Width, desc.Height)), desc.Format, 0, 0, None)
}

/// A texture of `size` in `format`, for `bind` (none: a staging texture the processor reads), with the `misc` flags,
/// filled from `init` (its bytes, and the length of a row) when given.
pub fn texture(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    format: DXGI_FORMAT,
    bind: u32,
    misc: u32,
    init: Option<(&[u8], u32)>,
) -> Result<ID3D11Texture2D, D3dError> {
    let staging = bind == 0;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: if staging { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
        BindFlags: bind,
        CPUAccessFlags: if staging { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
        MiscFlags: misc,
    };
    if init.is_some_and(|(bytes, pitch)| bytes.len() < pitch as usize * height as usize) {
        return Err(D3dError::Missing("CreateTexture2D: too few bytes"));
    }
    let init = init.map(|(bytes, pitch)| D3D11_SUBRESOURCE_DATA {
        pSysMem: bytes.as_ptr().cast(),
        SysMemPitch: pitch,
        SysMemSlicePitch: 0,
    });
    let mut out = None;
    // SAFETY: `desc` describes a plain texture; `init` points at `height` rows of `pitch` bytes, which outlive the
    // call; the out-pointer is valid.
    unsafe { device.CreateTexture2D(&raw const desc, init.as_ref().map(|i| i as *const _), Some(&raw mut out)) }
        .map_err(call("CreateTexture2D"))?;
    out.ok_or(D3dError::Missing("CreateTexture2D: no texture"))
}
